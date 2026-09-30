//! `ipgen-mcp` — a Model Context Protocol server over stdio exposing the
//! ipgen generator as tools. One JSON-RPC 2.0 message per line on stdin;
//! responses (and notifications) one per line on stdout. Logs go to stderr.
//!
//! Sessions live in memory (`HashMap<String, Session>`); each session holds a
//! live `Generator` plus its last `Checkpoint`.

use ipgen_core::categories::{ranges_for, Category};
use ipgen_core::checkpoint::{new_session_id, Checkpoint};
use ipgen_core::config::{Config, Preset};
use ipgen_core::generator::Generator;
use ipgen_core::order::{Order, Shard, DEFAULT_BLOCK_PREFIX};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "ipgen-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Session {
    gen: Generator,
    ckpt: Checkpoint,
}

pub fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut sessions: HashMap<String, Session> = HashMap::new();
    let mut lock = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[{SERVER_NAME}] stdin error: {e}");
                break;
            }
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                // unparseable garbage -> JSON-RPC parse error with id null
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": "parse error"}
                });
                writeln!(lock, "{resp}").ok();
                lock.flush().ok();
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        // notifications (no id) get no response
        let is_notification = req.get("id").is_none();

        let result = dispatch(method, &params, &mut sessions);
        if is_notification {
            continue;
        }
        let resp = match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err((code, msg)) => json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": code, "message": msg}}),
        };
        if writeln!(lock, "{resp}").is_err() {
            eprintln!("[{SERVER_NAME}] stdout closed; exiting");
            break;
        }
        lock.flush().ok();
    }
}

type RpcResult = Result<Value, (i64, String)>;

fn dispatch(method: &str, params: &Value, sessions: &mut HashMap<String, Session>) -> RpcResult {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_defs()})),
        "tools/call" => call_tool(params, sessions),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

fn tool_defs() -> Vec<Value> {
    let cfg_schema = json!({
        "type": "object",
        "properties": {
            "preset": {"type": "string", "enum": ["public", "private", "all", "public+private"]},
            "categories": {"type": "array", "items": {"type": "string"}},
            "include": {"type": "array", "items": {"type": "string"}},
            "exclude": {"type": "array", "items": {"type": "string"}},
            "order": {"type": "string", "enum": ["sequential", "permuted", "blocks"]},
            "seed": {"type": "integer"},
            "prefix_len": {"type": "integer", "minimum": 8, "maximum": 32},
            "shard": {"type": "string", "pattern": "^[0-9]+/[0-9]+$"}
        }
    });
    vec![
        json!({"name":"generate_start","description":"Start a generation session from a config object; returns session_id and totals.","inputSchema":json!({"type":"object","properties":{"config":cfg_schema},"required":["config"]})}),
        json!({"name":"generate_next","description":"Pull the next batch of items (addresses or /prefix blocks depending on order mode).","inputSchema":json!({"type":"object","properties":{"session_id":{"type":"string"},"count":{"type":"integer","minimum":1,"maximum":100000}},"required":["session_id","count"]})}),
        json!({"name":"generate_status","description":"Get session progress: cursor, total, remaining, percent.","inputSchema":json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]})}),
        json!({"name":"generate_pause","description":"Pause a session and return its checkpoint (JSON). The session stays in memory until closed.","inputSchema":json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]})}),
        json!({"name":"generate_resume","description":"Resume a session from a checkpoint previously returned by generate_pause or exported.","inputSchema":json!({"type":"object","properties":{"checkpoint":{"type":"object"}},"required":["checkpoint"]})}),
        json!({"name":"ranges_list","description":"List the CIDR blocks for given categories.","inputSchema":json!({"type":"object","properties":{"categories":{"type":"array","items":{"type":"string"}}},"required":["categories"]})}),
        json!({"name":"category_of","description":"Classify an IPv4 address into a category.","inputSchema":json!({"type":"object","properties":{"address":{"type":"string"}},"required":["address"]})}),
        json!({"name":"generate_close","description":"Close and forget a session.","inputSchema":json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]})}),
    ]
}

fn text_payload(v: Value) -> Value {
    json!({"content":[{"type":"text","text":v.to_string()}],"isError":false})
}

fn err_payload(msg: impl Into<String>) -> Value {
    json!({"content":[{"type":"text","text":msg.into()}],"isError":true})
}

fn call_tool(params: &Value, sessions: &mut HashMap<String, Session>) -> RpcResult {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or((-32602, "missing tool name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let out = match name {
        "generate_start" => tool_generate_start(&args, sessions),
        "generate_next" => tool_generate_next(&args, sessions),
        "generate_status" => tool_generate_status(&args, sessions),
        "generate_pause" => tool_generate_pause(&args, sessions),
        "generate_resume" => tool_generate_resume(&args, sessions),
        "ranges_list" => tool_ranges_list(&args),
        "category_of" => tool_category_of(&args),
        "generate_close" => tool_generate_close(&args, sessions),
        other => return Err((-32602, format!("unknown tool: {other}"))),
    };
    Ok(match out {
        Ok(v) => text_payload(v),
        Err(e) => err_payload(e.to_string()),
    })
}

fn build_config_from_json(cfg: &Value) -> Result<Config, ipgen_core::Error> {
    let mut b = Config::builder();
    if let Some(cats) = cfg.get("categories").and_then(|c| c.as_array()) {
        let mut parsed = Vec::new();
        for c in cats {
            parsed.push(Category::parse(c.as_str().unwrap_or(""))?);
        }
        b = b.categories(&parsed);
    } else if let Some(p) = cfg.get("preset").and_then(|p| p.as_str()) {
        b = b.preset(Preset::parse(p)?);
    }
    if let Some(inc) = cfg.get("include").and_then(|c| c.as_array()) {
        for i in inc {
            if let Some(s) = i.as_str() {
                b = b.include(s);
            }
        }
    }
    if let Some(exc) = cfg.get("exclude").and_then(|c| c.as_array()) {
        for i in exc {
            if let Some(s) = i.as_str() {
                b = b.exclude(s);
            }
        }
    }
    let seed = cfg.get("seed").and_then(|s| s.as_u64()).unwrap_or(0);
    let prefix_len = cfg.get("prefix_len").and_then(|s| s.as_u64()).map(|p| p as u8);
    let order = match cfg.get("order").and_then(|o| o.as_str()).unwrap_or("permuted") {
        "sequential" => Order::Sequential,
        "blocks" => Order::Blocks { seed, prefix_len },
        _ => Order::Permuted { seed },
    };
    b = b.order(order);
    if let Some(sh) = cfg.get("shard").and_then(|s| s.as_str()) {
        b = b.shard(Shard::parse(sh)?);
    }
    b.build()
}

fn tool_generate_start(args: &Value, sessions: &mut HashMap<String, Session>) -> Result<Value, ipgen_core::Error> {
    let cfgv = args
        .get("config")
        .filter(|c| c.is_object())
        .ok_or_else(|| ipgen_core::Error::CheckpointInvalid("missing 'config' object".into()))?;
    let cfg = build_config_from_json(cfgv)?;
    let total_allowed = cfg.total_allowed()?;
    let shard_total = cfg.shard_total()?;
    let g = Generator::new(cfg)?;
    let ckpt = g.checkpoint();
    let sid = new_session_id(ckpt.created_at);
    let ckpt = ckpt.clone().with_session_id(sid.clone());
    let mut g2 = Generator::resume(ckpt.clone())?;
    // fresh generator already at cursor 0; keep original but fix session id
    g2.seek(0)?;
    sessions.insert(
        sid.clone(),
        Session {
            gen: g2,
            ckpt: ckpt.clone(),
        },
    );
    Ok(json!({
        "session_id": sid,
        "total_addresses": total_allowed,
        "shard_total": shard_total,
        "order": ckpt.config.order.name(),
        "config_hash": format!("{:#018x}", ckpt.config_hash),
    }))
}

fn session<'a>(
    sessions: &'a mut HashMap<String, Session>,
    args: &Value,
) -> Result<&'a mut Session, ipgen_core::Error> {
    let sid = args
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or_else(|| ipgen_core::Error::CheckpointInvalid("missing session_id".into()))?;
    sessions
        .get_mut(sid)
        .ok_or_else(|| ipgen_core::Error::CheckpointInvalid(format!("unknown session: {sid}")))
}

fn tool_generate_next(args: &Value, sessions: &mut HashMap<String, Session>) -> Result<Value, ipgen_core::Error> {
    let count = args
        .get("count")
        .and_then(|c| c.as_u64())
        .filter(|c| *c >= 1 && *c <= 100_000)
        .ok_or_else(|| ipgen_core::Error::IndexOutOfRange { index: 0, total: 100_000 })? as usize;
    let s = session(sessions, args)?;
    let is_blocks = matches!(s.gen.config().order, Order::Blocks { .. });
    let start = s.gen.cursor();
    if is_blocks {
        let plen = match s.gen.config().order {
            Order::Blocks { prefix_len, .. } => prefix_len.unwrap_or(DEFAULT_BLOCK_PREFIX),
            _ => unreachable!(),
        };
        let blocks = s.gen.next_cidr_blocks(count, plen)?;
        let items: Vec<Value> = blocks
            .iter()
            .enumerate()
            .map(|(k, b)| json!({"index": start + k as u64, "cidr": b.to_cidr_string(), "count": b.count}))
            .collect();
        s.ckpt = s.gen.checkpoint().with_session_id(s.ckpt.session_id.clone());
        Ok(json!({"items": items, "cursor": s.gen.cursor(), "total": s.gen.total()}))
    } else {
        let mut buf = vec![0u32; count];
        let got = s.gen.next_batch_into(&mut buf);
        let items: Vec<Value> = buf[..got]
            .iter()
            .enumerate()
            .map(|(k, v)| {
                let ip = std::net::Ipv4Addr::from(*v);
                json!({"index": start + k as u64, "address": ip.to_string(),
                       "category": ipgen_core::classify(ip).name()})
            })
            .collect();
        s.ckpt = s.gen.checkpoint().with_session_id(s.ckpt.session_id.clone());
        Ok(json!({"items": items, "cursor": s.gen.cursor(), "total": s.gen.total()}))
    }
}

fn tool_generate_status(args: &Value, sessions: &mut HashMap<String, Session>) -> Result<Value, ipgen_core::Error> {
    let s = session(sessions, args)?;
    let total = s.gen.total();
    let cursor = s.gen.cursor();
    Ok(json!({
        "session_id": s.ckpt.session_id,
        "cursor": cursor,
        "total": total,
        "remaining": total.saturating_sub(cursor),
        "percent_complete": if total > 0 { cursor as f64 * 100.0 / total as f64 } else { 0.0 },
        "order": s.gen.config().order.name(),
        "config_hash": format!("{:#018x}", s.ckpt.config_hash),
    }))
}

fn tool_generate_pause(args: &Value, sessions: &mut HashMap<String, Session>) -> Result<Value, ipgen_core::Error> {
    let s = session(sessions, args)?;
    s.ckpt = s.gen.checkpoint().with_session_id(s.ckpt.session_id.clone());
    serde_json::to_value(&s.ckpt).map_err(|e| ipgen_core::Error::Io(e.to_string()))
}

fn tool_generate_resume(args: &Value, sessions: &mut HashMap<String, Session>) -> Result<Value, ipgen_core::Error> {
    let ckv = args
        .get("checkpoint")
        .cloned()
        .unwrap_or(Value::Null);
    if !ckv.is_object() {
        return Err(ipgen_core::Error::CheckpointInvalid(
            "missing 'checkpoint' object".into(),
        ));
    }
    let ckpt: Checkpoint = serde_json::from_value(ckv)
        .map_err(|e| ipgen_core::Error::CheckpointInvalid(format!("bad checkpoint: {e}")))?;
    ckpt.validate()?;
    let sid = ckpt.session_id.clone();
    let gen = Generator::resume(ckpt.clone())?;
    let cursor = gen.cursor();
    let total = gen.total();
    sessions.insert(sid.clone(), Session { gen, ckpt });
    Ok(json!({"session_id": sid, "cursor": cursor, "total": total}))
}

fn tool_ranges_list(args: &Value) -> Result<Value, ipgen_core::Error> {
    let cats_v = args
        .get("categories")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let mut cats = Vec::new();
    for c in &cats_v {
        cats.push(Category::parse(c.as_str().unwrap_or(""))?);
    }
    if cats.is_empty() {
        cats = Category::ALL.to_vec();
    }
    let set = ranges_for(&cats);
    let cidrs: Vec<String> = set
        .to_cidrs()
        .iter()
        .map(|(a, p)| format!("{a}/{p}"))
        .collect();
    Ok(json!({"cidrs": cidrs, "total_addresses": set.len()}))
}

fn tool_category_of(args: &Value) -> Result<Value, ipgen_core::Error> {
    let s = args
        .get("address")
        .and_then(|a| a.as_str())
        .ok_or_else(|| ipgen_core::Error::BadAddress("missing address".into()))?;
    let addr: std::net::Ipv4Addr = s.parse().map_err(|_| ipgen_core::Error::BadAddress(s.into()))?;
    Ok(json!({"address": addr.to_string(), "category": ipgen_core::classify(addr).name()}))
}

fn tool_generate_close(args: &Value, sessions: &mut HashMap<String, Session>) -> Result<Value, ipgen_core::Error> {
    let sid = args
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or_else(|| ipgen_core::Error::CheckpointInvalid("missing session_id".into()))?
        .to_string();
    match sessions.remove(&sid) {
        Some(_) => Ok(json!({"closed": sid})),
        None => Err(ipgen_core::Error::CheckpointInvalid(format!(
            "unknown session: {sid}"
        ))),
    }
}
