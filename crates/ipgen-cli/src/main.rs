//! `ipgen` — algorithmic IPv4 range generator with pause/resume.
//!
//! This tool only *enumerates* addresses. It performs no network I/O of any
//! kind. Use output only against networks you own or are authorized to examine.

use clap::{Args, Parser, Subcommand};
use ipgen_core::categories::{classify, ranges_for, Category};
use ipgen_core::checkpoint::Checkpoint;
use ipgen_core::config::{Config, Preset};
use ipgen_core::interval::IntervalSet;
use ipgen_core::order::{Order, Shard, DEFAULT_BLOCK_PREFIX};
use ipgen_core::Generator;
use std::io::{self, BufWriter, Write};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "ipgen",
    version,
    about = "Algorithmic IPv4 range/CIDR generator with exact pause & resume.",
    long_about = "ipgen enumerates IPv4 addresses, CIDRs and /prefix blocks from a \
                  declarative config. It performs NO network I/O: no sockets, no probing, \
                  no scanning. Only use generated addresses against networks you own or \
                  are explicitly authorized to examine."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a stream of addresses / CIDRs.
    Generate(GenerateArgs),
    /// Resume generation from a checkpoint file.
    Resume(ResumeArgs),
    /// Show session state (cursor, total, progress) without generating.
    Status(StatusArgs),
    /// List the built-in category tables and sizes.
    Categories(CategoriesArgs),
    /// Print the maximal CIDR set for a category or custom include/exclude.
    Ranges(RangesArgs),
}

#[derive(Args)]
struct SelectionArgs {
    /// preset: public | private | all | public+private
    #[arg(long, default_value = "public")]
    preset: String,
    /// explicit comma-separated category list (overrides --preset)
    #[arg(long, value_delimiter = ',')]
    categories: Vec<String>,
    /// extra included CIDR/range/address (repeatable)
    #[arg(long)]
    include: Vec<String>,
    /// extra excluded CIDR/range/address (repeatable)
    #[arg(long)]
    exclude: Vec<String>,
}

#[derive(Args)]
struct GenerateArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// ordering: sequential | permuted | blocks
    #[arg(long, default_value = "permuted")]
    order: String,
    /// seed for permuted/blocks ordering
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// prefix length for blocks mode
    #[arg(long)]
    prefix_len: Option<u8>,
    /// shard K/M (worker K of M)
    #[arg(long, default_value = "0/1")]
    shard: String,
    /// how many items to emit (default: until Ctrl-C)
    #[arg(long)]
    count: Option<u64>,
    /// batch size per internal call
    #[arg(long, default_value_t = 4096)]
    batch: usize,
    /// output format: text | jsonl | csv
    #[arg(long, default_value = "text")]
    format: String,
    /// write to FILE instead of stdout
    #[arg(long)]
    out: Option<PathBuf>,
    /// also write a checkpoint file every N batches (0 = only at exit)
    #[arg(long, default_value_t = 0)]
    checkpoint_every: u64,
    /// path for the checkpoint file
    #[arg(long, default_value = "ipgen.state.json")]
    state: PathBuf,
}

#[derive(Args)]
struct ResumeArgs {
    /// checkpoint file written by a previous run
    #[arg(long, default_value = "ipgen.state.json")]
    state: PathBuf,
    #[arg(long, default_value_t = 4096)]
    batch: usize,
    #[arg(long)]
    count: Option<u64>,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    checkpoint_every: u64,
}

#[derive(Args)]
struct StatusArgs {
    #[arg(long, default_value = "ipgen.state.json")]
    state: PathBuf,
}

#[derive(Args)]
struct CategoriesArgs {
    /// show individual CIDR entries per category
    #[arg(long, default_value_t = false)]
    detailed: bool,
}

#[derive(Args)]
struct RangesArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// print as compact table instead of full CIDR list
    #[arg(long, default_value_t = false)]
    summary: bool,
}

fn build_config(sel: &SelectionArgs, order: &str, seed: u64, shard: &str) -> Result<Config, String> {
    let mut b = Config::builder();
    if sel.categories.is_empty() {
        let p = Preset::parse(&sel.preset).map_err(|e| e.to_string())?;
        b = b.preset(p);
    } else {
        let mut cats = Vec::new();
        for c in &sel.categories {
            cats.push(Category::parse(c).map_err(|e| e.to_string())?);
        }
        b = b.categories(&cats);
    }
    for i in &sel.include {
        b = b.include(i.clone());
    }
    for e in &sel.exclude {
        b = b.exclude(e.clone());
    }
    let ord = match order {
        "sequential" => Order::Sequential,
        "permuted" => Order::Permuted { seed },
        "blocks" => Order::Blocks { seed, prefix_len: None },
        other => return Err(format!("unknown --order '{other}' (sequential|permuted|blocks)")),
    };
    b = b.order(ord);
    b = b.shard(Shard::parse(shard).map_err(|e| e.to_string())?);
    b.build().map_err(|e| e.to_string())
}

struct Emitter {
    format: Format,
    w: Box<dyn Write>,
}

#[derive(PartialEq, Clone, Copy)]
enum Format {
    Text,
    Jsonl,
    Csv,
}

impl Emitter {
    fn new(format: &str, out: &Option<PathBuf>) -> io::Result<Emitter> {
        let fmt = match format {
            "text" => Format::Text,
            "jsonl" => Format::Jsonl,
            "csv" => Format::Csv,
            other => {
                eprintln!("error: unknown --format '{other}' (text|jsonl|csv)");
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad format"));
            }
        };
        let w: Box<dyn Write> = match out {
            Some(p) => {
                let f = std::fs::File::create(p)?;
                Box::new(BufWriter::new(f))
            }
            None => Box::new(BufWriter::new(io::stdout())),
        };
        Ok(Emitter { format: fmt, w })
    }

    fn addr(&mut self, idx: u64, ip: Ipv4Addr) -> io::Result<()> {
        match self.format {
            Format::Text => writeln!(self.w, "{ip}"),
            Format::Jsonl => writeln!(
                self.w,
                "{{\"index\":{idx},\"address\":\"{ip}\",\"category\":\"{}\"}}",
                classify(ip).name()
            ),
            Format::Csv => writeln!(self.w, "{idx},{ip},{}", classify(ip).name()),
        }
    }

    fn block(&mut self, idx: u64, cidr: &str, count: u64) -> io::Result<()> {
        match self.format {
            Format::Text => writeln!(self.w, "{cidr}"),
            Format::Jsonl => writeln!(
                self.w,
                "{{\"index\":{idx},\"cidr\":\"{cidr}\",\"count\":{count}}}"
            ),
            Format::Csv => writeln!(self.w, "{idx},{cidr},{count}"),
        }
    }

    fn csv_header_addr(&mut self) -> io::Result<()> {
        if self.format == Format::Csv {
            writeln!(self.w, "index,address,category")?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Run generation loop for an existing generator; returns exit code.
fn run_stream(
    g: &mut Generator,
    args_order: &str,
    prefix_len: u8,
    count: Option<u64>,
    batch: usize,
    em: &mut Emitter,
    state_path: &PathBuf,
    checkpoint_every: u64,
    stop: &AtomicBool,
) -> i32 {
    let total_to_emit = match count {
        Some(c) => c.min(g.remaining()),
        None => g.remaining(),
    };
    let start_cursor = g.cursor();
    em.csv_header_addr().ok();
    let mut buf = vec![0u32; batch.max(1).min(1 << 20)];
    let mut emitted: u64 = 0;
    let mut batches_since_ckpt: u64 = 0;
    let is_blocks = args_order == "blocks";
    let mut rc = 0i32;

    while emitted < total_to_emit && !stop.load(AtomicOrdering::Relaxed) {
        let want = (total_to_emit - emitted).min(batch as u64) as usize;
        if is_blocks {
            match g.next_cidr_blocks(want, prefix_len) {
                Ok(blocks) => {
                    if blocks.is_empty() {
                        break;
                    }
                    for blk in blocks {
                        let _ = em.block(start_cursor + emitted, &blk.to_cidr_string(), blk.count);
                        emitted += 1;
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    rc = 2;
                    break;
                }
            }
        } else {
            let got = g.next_batch_into(&mut buf[..want]);
            if got == 0 {
                break;
            }
            for k in 0..got {
                let ip = Ipv4Addr::from(buf[k]);
                if em.addr(start_cursor + emitted, ip).is_err() {
                    // broken pipe etc: stop gracefully
                    rc = 3;
                    break;
                }
                emitted += 1;
            }
            if rc != 0 {
                break;
            }
        }
        batches_since_ckpt += 1;
        if checkpoint_every > 0 && batches_since_ckpt >= checkpoint_every {
            save_state(g, state_path);
            batches_since_ckpt = 0;
        }
        let _ = em.flush();
    }
    let _ = em.flush();
    // final checkpoint (exact position where we stopped)
    save_state(g, state_path);
    if stop.load(AtomicOrdering::Relaxed) {
        eprintln!(
            "\nipgen: paused after {emitted} more items (cursor {}/{}) — state saved to {}",
            g.cursor(),
            g.total(),
            state_path.display()
        );
    }
    rc
}

fn save_state(g: &Generator, path: &PathBuf) {
    let ck = g.checkpoint();
    if let Err(e) = ck.save_atomic(path) {
        eprintln!("warning: could not save checkpoint: {e}");
    }
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Cmd::Generate(a) => cmd_generate(a),
        Cmd::Resume(a) => cmd_resume(a),
        Cmd::Status(a) => cmd_status(a),
        Cmd::Categories(a) => cmd_categories(a),
        Cmd::Ranges(a) => cmd_ranges(a),
    };
    std::process::exit(code);
}

fn cmd_generate(a: GenerateArgs) -> i32 {
    let cfg = match build_config(&a.selection, &a.order, a.seed, &a.shard) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut g = match Generator::new(cfg) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let prefix_len = a.prefix_len.unwrap_or(DEFAULT_BLOCK_PREFIX);
    let mut em = match Emitter::new(&a.format, &a.out) {
        Ok(e) => e,
        Err(_) => return 2,
    };
    let stop = install_ctrlc();
    run_stream(
        &mut g,
        &a.order,
        prefix_len,
        a.count,
        a.batch,
        &mut em,
        &a.state,
        a.checkpoint_every,
        &stop,
    )
}

fn cmd_resume(a: ResumeArgs) -> i32 {
    let ck = match Checkpoint::load(&a.state) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let order_name = ck.config.order.name().to_string();
    let prefix_len = match ck.config.order {
        Order::Blocks { prefix_len, .. } => prefix_len.unwrap_or(DEFAULT_BLOCK_PREFIX),
        _ => DEFAULT_BLOCK_PREFIX,
    };
    let mut g = match Generator::resume(ck) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut em = match Emitter::new(&a.format, &a.out) {
        Ok(e) => e,
        Err(_) => return 2,
    };
    let stop = install_ctrlc();
    run_stream(
        &mut g,
        &order_name,
        prefix_len,
        a.count,
        a.batch,
        &mut em,
        &a.state,
        a.checkpoint_every,
        &stop,
    )
}

fn cmd_status(a: StatusArgs) -> i32 {
    let ck = match Checkpoint::load(&a.state) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Err(e) = ck.validate() {
        eprintln!("error: {e}");
        return 2;
    }
    let done = ck.cursor;
    let total = ck.total;
    let pct = if total > 0 {
        (done as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    println!("session_id:  {}", ck.session_id);
    println!("config_hash: {:#018x}", ck.config_hash);
    println!("order:       {}", ck.config.order.name());
    println!(
        "shard:       {}/{}",
        ck.config.shard.index, ck.config.shard.count
    );
    println!("cursor:      {done} / {total} ({pct:.4}% complete)");
    println!("remaining:   {}", total.saturating_sub(done));
    println!("created_at:  {}", ck.created_at);
    println!("updated_at:  {}", ck.updated_at);
    0
}

fn cmd_categories(a: CategoriesArgs) -> i32 {
    let mut rows: Vec<(String, u64)> = Vec::new();
    for cat in Category::ALL {
        let set = ranges_for(&[*cat]);
        rows.push((cat.name().to_string(), set.len()));
    }
    let total: u64 = rows.iter().map(|(_, n)| n).sum();
    for (name, n) in &rows {
        println!("{name:<16} {n:>12}  {:.4}%", (*n as f64 / total as f64) * 100.0);
    }
    println!("{:<16} {total:>12}", "TOTAL");
    if a.detailed {
        println!();
        for cat in Category::ALL {
            let set = ranges_for(&[*cat]);
            let cidrs: Vec<String> = set
                .to_cidrs()
                .into_iter()
                .map(|(a, p)| format!("{a}/{p}"))
                .collect();
            println!("{}:", cat.name());
            for line_chunk in cidrs.chunks(6) {
                println!("  {}", line_chunk.join(" "));
            }
        }
    }
    0
}

fn cmd_ranges(a: RangesArgs) -> i32 {
    let cfg = match build_config(&a.selection, "sequential", 0, "0/1") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let set: IntervalSet = match cfg.interval_set() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if a.summary {
        println!("{}", serde_json::to_string_pretty(&summary_json(&set)).unwrap());
    } else {
        for (addr, plen) in set.to_cidrs() {
            println!("{addr}/{plen}");
        }
    }
    0
}

fn summary_json(set: &IntervalSet) -> serde_json::Value {
    serde_json::json!({
        "cidr_count": set.to_cidrs().len(),
        "total_addresses": set.len(),
    })
}

fn install_ctrlc() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let _ = ctrlc::set_handler(move || {
        s.store(true, AtomicOrdering::Relaxed);
    });
    stop
}
