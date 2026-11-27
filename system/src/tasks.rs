//! saule-tasks — what is running, and how hard the machine is working.
//!
//! For Task Manager, which is a Saule program run as the person's account:
//! reading a hundred processes' files out of `/proc` and working their
//! shares out every other second is quick here and slow there, so this
//! does it and prints the answer, a line a thing, for the app to show.
//!
//!   saule-tasks watch [SECONDS]   a sample now and every SECONDS after
//!                                 (2 unless said), until whoever is
//!                                 reading goes away
//!   saule-tasks once              one sample: shares since the machine
//!                                 started, for looking at by hand
//!   saule-tasks quit PID          ask a process to end (SIGTERM)
//!   saule-tasks kill PID          end it (SIGKILL)
//!
//! It runs as whoever starts it and can end only what that account may:
//! the person's own apps, not the system's services. Refused, it says why
//! and exits 1.
//!
//! A sample is these lines, their fields set apart by tabs, then `end`:
//!
//!   me    UID                          the account this runs as
//!   cpu   BUSY USER SYSTEM CORES LOAD   shares of all the processors
//!                                      together, in tenths of a percent;
//!                                      how many there are; the load
//!                                      average over a minute
//!   core  N BUSY                       one processor's share, likewise
//!   mem   TOTAL USED CACHED AVAILABLE SWAP SWAPUSED       in kB
//!   gpu   BUSY VRAMUSED VRAMTOTAL      percent and kB; -1 for what the
//!                                      graphics chip does not say
//!   p     PID PPID UID USER KIND CPU RSS THREADS STATE NAME COMMAND
//!                                      CPU in tenths of a percent of one
//!                                      processor, RSS in kB
//!
//! KIND is what the process is to a person: `a` an app (a program out of
//! the apps' folder), `w` the session, `s` a service (started with the
//! machine, by init or by what init started), `k` one of the kernel's own
//! threads, `p` anything else (what an app started: a shell in a
//! terminal).

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant};

/// Where Saule Linux keeps its apps, unless `SAULE_APPS` names another.
const APPS: &str = "/bin/apps";
const SESSION: &str = "/bin/session";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("watch") => {
            let every = args
                .get(1)
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|s| s.is_finite() && *s >= 0.2)
                .unwrap_or(2.0);
            watch(Duration::from_secs_f64(every))
        }
        Some("once") => {
            let mut sampler = Sampler::new();
            i32::from(!sampler.print())
        }
        Some("quit") => signal(args.get(1), libc::SIGTERM),
        Some("kill") => signal(args.get(1), libc::SIGKILL),
        _ => {
            eprintln!("usage: saule-tasks watch [SECONDS] | once | quit PID | kill PID");
            2
        }
    };
    std::process::exit(code);
}

/// A sample now and every `every` after, until nobody is reading: the
/// app has closed, or ended without closing anything.
fn watch(every: Duration) -> i32 {
    let mut sampler = Sampler::new();
    // The first sample has nothing before it to compare with: taken, and
    // not shown, so that the first one shown has true shares in it.
    sampler.take();
    std::thread::sleep(Duration::from_millis(250));
    loop {
        if !sampler.print() {
            return 0;
        }
        std::thread::sleep(every);
        // Left behind by an app that ended: init has it now.
        if unsafe { libc::getppid() } == 1 {
            return 0;
        }
    }
}

fn signal(pid: Option<&String>, signal: libc::c_int) -> i32 {
    let Some(pid) = pid.and_then(|p| p.parse::<i32>().ok()).filter(|p| *p > 1) else {
        eprintln!("saule-tasks: which process? Give its number.");
        return 2;
    };
    if unsafe { libc::kill(pid, signal) } == 0 {
        return 0;
    }
    let error = std::io::Error::last_os_error();
    let why = match error.raw_os_error() {
        Some(libc::EPERM) => "It belongs to the system and cannot be ended from here.".to_string(),
        Some(libc::ESRCH) => "It has already ended.".to_string(),
        _ => error.to_string(),
    };
    eprintln!("{why}");
    1
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// A processor's time so far, in the kernel's ticks: all of it, the part
/// spent doing nothing, and the parts spent in programs and in the kernel.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Ticks {
    total: u64,
    idle: u64,
    user: u64,
    system: u64,
}

/// One process, as `/proc` says it is now.
#[derive(Debug, Default, Clone, PartialEq)]
struct Process {
    pid: i32,
    ppid: i32,
    uid: u32,
    state: char,
    /// Time it has had a processor for, in ticks, and when it started (the
    /// two together tell a process from another given its number later).
    ticks: u64,
    started: u64,
    rss_kb: u64,
    threads: u64,
    name: String,
    command: String,
    /// The program it is, where that can be read.
    program: String,
}

struct Sampler {
    all: Ticks,
    cores: Vec<Ticks>,
    /// Each process's ticks at the last sample, by number and start.
    seen: HashMap<(i32, u64), u64>,
    at: Instant,
    per_second: f64,
    page_kb: u64,
    users: HashMap<u32, String>,
    apps: String,
}

impl Sampler {
    fn new() -> Sampler {
        let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Sampler {
            all: Ticks::default(),
            cores: Vec::new(),
            seen: HashMap::new(),
            at: Instant::now(),
            per_second: if per_second > 0 { per_second as f64 } else { 100.0 },
            page_kb: if page > 0 { page as u64 / 1024 } else { 4 },
            users: users(&fs::read_to_string("/etc/passwd").unwrap_or_default()),
            apps: apps_folder(std::env::var("SAULE_APPS").ok()),
        }
    }

    /// Take a sample and print it. `false` when nobody is reading.
    fn print(&mut self) -> bool {
        let text = self.take();
        let mut out = std::io::stdout().lock();
        out.write_all(text.as_bytes()).is_ok() && out.flush().is_ok()
    }

    /// A sample, as text: shares are of the time since the last one.
    fn take(&mut self) -> String {
        let mut text = String::with_capacity(16 * 1024);
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_secs_f64().max(0.001);
        self.at = now;

        // Whose the reader's own processes are: the ones it may end.
        text.push_str(&format!("me\t{}\n", unsafe { libc::getuid() }));

        // The processors.
        let (all, cores) = processors(&fs::read_to_string("/proc/stat").unwrap_or_default());
        let load = fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|l| l.split_whitespace().next().map(str::to_string))
            .unwrap_or_else(|| "0".to_string());
        let (busy, user, system) = shares(self.all, all);
        text.push_str(&format!("cpu\t{busy}\t{user}\t{system}\t{}\t{load}\n", cores.len().max(1)));
        for (i, core) in cores.iter().enumerate() {
            let before = self.cores.get(i).copied().unwrap_or_default();
            text.push_str(&format!("core\t{i}\t{}\n", shares(before, *core).0));
        }
        self.all = all;
        self.cores = cores;

        // Memory.
        let m = memory(&fs::read_to_string("/proc/meminfo").unwrap_or_default());
        text.push_str(&format!(
            "mem\t{}\t{}\t{}\t{}\t{}\t{}\n",
            m.total,
            m.total.saturating_sub(m.available),
            m.cached,
            m.available,
            m.swap,
            m.swap.saturating_sub(m.swap_free)
        ));

        // The graphics chip, where it says.
        let (gpu, vram_used, vram) = graphics();
        text.push_str(&format!("gpu\t{gpu}\t{vram_used}\t{vram}\n"));

        // The processes.
        let processes = processes(self.page_kb);
        let kinds = kinds(&processes, &self.apps);
        let mut seen = HashMap::with_capacity(processes.len());
        for p in &processes {
            let key = (p.pid, p.started);
            let before = self.seen.get(&key).copied().unwrap_or(p.ticks);
            let used = p.ticks.saturating_sub(before) as f64 / self.per_second;
            let share = (used / elapsed * 1000.0).round() as u64;
            seen.insert(key, p.ticks);
            let user = match self.users.get(&p.uid) {
                Some(name) => name.clone(),
                None => p.uid.to_string(),
            };
            text.push_str(&format!(
                "p\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                p.pid,
                p.ppid,
                p.uid,
                user,
                kinds.get(&p.pid).copied().unwrap_or('p'),
                share,
                p.rss_kb,
                p.threads,
                p.state,
                clean(&p.name),
                clean(&p.command)
            ));
        }
        self.seen = seen;
        text.push_str("end\n");
        text
    }
}

/// A field with nothing in it that would end the field or the line.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// The shares of the time between two readings spent busy, in programs
/// and in the kernel, in tenths of a percent.
fn shares(before: Ticks, now: Ticks) -> (u64, u64, u64) {
    let total = now.total.saturating_sub(before.total);
    if total == 0 || before.total == 0 {
        return (0, 0, 0);
    }
    let part = |ticks: u64| (ticks as f64 * 1000.0 / total as f64).round() as u64;
    let idle = now.idle.saturating_sub(before.idle).min(total);
    (
        part(total - idle),
        part(now.user.saturating_sub(before.user).min(total)),
        part(now.system.saturating_sub(before.system).min(total)),
    )
}

/// `/proc/stat`: all the processors together, and each.
fn processors(stat: &str) -> (Ticks, Vec<Ticks>) {
    let mut all = Ticks::default();
    let mut cores = Vec::new();
    for line in stat.lines() {
        let mut words = line.split_whitespace();
        let Some(name) = words.next() else { continue };
        if !name.starts_with("cpu") {
            continue;
        }
        let n: Vec<u64> = words.filter_map(|w| w.parse().ok()).collect();
        if n.len() < 4 {
            continue;
        }
        // user nice system idle iowait irq softirq steal: waiting for a
        // disk is not work, so it counts with idle.
        let ticks = Ticks {
            total: n.iter().take(8).sum(),
            idle: n[3] + n.get(4).copied().unwrap_or(0),
            user: n[0] + n[1],
            system: n[2] + n.get(5).copied().unwrap_or(0) + n.get(6).copied().unwrap_or(0),
        };
        if name == "cpu" {
            all = ticks;
        } else {
            cores.push(ticks);
        }
    }
    (all, cores)
}

#[derive(Default, Debug, PartialEq)]
struct Memory {
    total: u64,
    available: u64,
    cached: u64,
    swap: u64,
    swap_free: u64,
}

/// `/proc/meminfo`, in kB.
fn memory(info: &str) -> Memory {
    let mut m = Memory::default();
    let mut free = 0;
    let mut has_available = false;
    for line in info.lines() {
        let mut words = line.split_whitespace();
        let (Some(name), Some(value)) = (words.next(), words.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else { continue };
        match name {
            "MemTotal:" => m.total = value,
            "MemFree:" => free = value,
            "MemAvailable:" => {
                m.available = value;
                has_available = true;
            }
            "Cached:" | "SReclaimable:" => m.cached += value,
            "SwapTotal:" => m.swap = value,
            "SwapFree:" => m.swap_free = value,
            _ => {}
        }
    }
    if !has_available {
        m.available = free + m.cached;
    }
    m
}

/// How busy the graphics chip is, in percent, and its own memory in use
/// and in all, in kB: -1 for what it does not say. Few do: AMD's driver
/// has the files, Intel's has none of them.
fn graphics() -> (i64, i64, i64) {
    let number = |path: String| -> Option<i64> {
        fs::read_to_string(path).ok()?.trim().parse::<i64>().ok()
    };
    for card in 0..4 {
        let device = format!("/sys/class/drm/card{card}/device");
        if let Some(busy) = number(format!("{device}/gpu_busy_percent")) {
            let used = number(format!("{device}/mem_info_vram_used")).map_or(-1, |b| b / 1024);
            let total = number(format!("{device}/mem_info_vram_total")).map_or(-1, |b| b / 1024);
            return (busy.clamp(0, 100), used, total);
        }
    }
    (-1, -1, -1)
}

/// `/etc/passwd`: each account's name, by its number.
fn users(passwd: &str) -> HashMap<u32, String> {
    passwd
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

/// Every process there is, in order of its number.
fn processes(page_kb: u64) -> Vec<Process> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        let folder = format!("/proc/{pid}");
        // Gone between being listed and being read: not there.
        let Ok(stat) = fs::read_to_string(format!("{folder}/stat")) else {
            continue;
        };
        let Some(mut process) = parse_stat(&stat, page_kb) else {
            continue;
        };
        process.uid = fs::metadata(&folder).map(|m| m.uid()).unwrap_or(0);
        let command = fs::read(format!("{folder}/cmdline")).unwrap_or_default();
        let words: Vec<String> = command
            .split(|b| *b == 0)
            .filter(|w| !w.is_empty())
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
        // A Saule program is what Saule was given to run, not the Saule
        // that runs it, which is every app's alike. Any other: the program
        // itself where it may be read (one's own processes); else what it
        // was started as.
        let words = match saule_program(&words) {
            Some(own) => {
                process.program = own[0].clone();
                own
            }
            None => {
                process.program = fs::read_link(format!("{folder}/exe"))
                    .map(|p| p.to_string_lossy().into_owned())
                    .ok()
                    .or_else(|| words.first().cloned())
                    .unwrap_or_default();
                &words[..]
            }
        };
        process.name = named(&process.name, words);
        process.command = words.join(" ");
        out.push(process);
    }
    out.sort_by_key(|p| p.pid);
    out
}

/// The apps' folder as the file system has it: `named`, or else
/// [`APPS`], which on a stick and an installed system is a link to where
/// the apps are kept on the disk. Programs are known by where they are.
fn apps_folder(named: Option<String>) -> String {
    let folder = named.unwrap_or_else(|| APPS.to_string());
    match fs::canonicalize(&folder) {
        Ok(real) => real.to_string_lossy().into_owned(),
        Err(_) => folder,
    }
}

/// The words a Saule program was started with, itself first, out of the
/// words of its process: `/bin/saule run /bin/apps/editor notes.txt` is
/// `/bin/apps/editor notes.txt`. The apps are built without a runtime of
/// their own, and the system starts each as the one Saule running it.
fn saule_program(words: &[String]) -> Option<&[String]> {
    let [saule, run, own @ ..] = words else {
        return None;
    };
    let base = saule.rsplit('/').next().unwrap_or(saule);
    (base == "saule" && run == "run" && !own.is_empty()).then_some(own)
}

/// What a process is called: the kernel's name for it (`short`, its
/// first fifteen letters), made whole from what it was started as. One
/// of busybox's many tools is called by the tool's name, not the box's.
fn named(short: &str, words: &[String]) -> String {
    let Some(first) = words.first() else {
        return short.to_string();
    };
    let base = first.rsplit('/').next().unwrap_or(first);
    if base == "busybox" {
        if let Some(tool) = words.get(1).filter(|w| !w.starts_with('-')) {
            return tool.rsplit('/').next().unwrap_or(tool).to_string();
        }
    }
    if !base.is_empty() && base.starts_with(short) {
        return base.to_string();
    }
    short.to_string()
}

/// One process's `stat`. Its name is in brackets and may have brackets
/// and spaces of its own, so the fields are counted from the last `)`.
fn parse_stat(stat: &str, page_kb: u64) -> Option<Process> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let pid = stat[..open].trim().parse().ok()?;
    let name = stat.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // After the name: state(0) ppid(1) … utime(11) stime(12) …
    // num_threads(17) … starttime(19) vsize(20) rss(21).
    let number = |at: usize| rest.get(at).and_then(|w| w.parse::<u64>().ok());
    Some(Process {
        pid,
        ppid: rest.get(1)?.parse().ok()?,
        state: rest.first()?.chars().next()?,
        ticks: number(11)? + number(12)?,
        threads: number(17).unwrap_or(1),
        started: number(19).unwrap_or(0),
        rss_kb: number(21).unwrap_or(0) * page_kb,
        name,
        ..Process::default()
    })
}

/// What each process is to a person: see the top of the file.
fn kinds(processes: &[Process], apps: &str) -> HashMap<i32, char> {
    let by_pid: HashMap<i32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
    let folder = format!("{}/", apps.trim_end_matches('/'));
    let is_app = |p: &Process| p.program.starts_with(&folder);
    let is_session = |p: &Process| p.program == SESSION;
    let mut out = HashMap::with_capacity(processes.len());
    for p in processes {
        let kind = if p.command.is_empty() && (p.pid == 2 || p.ppid == 2) {
            'k'
        } else if is_app(p) {
            'a'
        } else if is_session(p) {
            'w'
        } else {
            // Up through what started it: under an app or the session it
            // is theirs; reaching init with neither on the way, it was
            // started with the machine.
            let mut kind = 's';
            let mut at = p.ppid;
            for _ in 0..64 {
                let Some(parent) = by_pid.get(&at) else { break };
                if is_app(parent) || is_session(parent) {
                    kind = 'p';
                    break;
                }
                if parent.pid <= 1 {
                    break;
                }
                at = parent.ppid;
            }
            kind
        };
        out.insert(p.pid, kind);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_with_brackets_and_spaces_in_it_is_read_whole() {
        let stat = "412 (my (odd) name) S 1 412 412 0 -1 4194304 100 0 0 0 7 5 0 0 20 0 3 0 9000 123456 250 18446744073709551615";
        let p = parse_stat(stat, 4).expect("read");
        assert_eq!(p.pid, 412);
        assert_eq!(p.name, "my (odd) name");
        assert_eq!(p.state, 'S');
        assert_eq!(p.ppid, 1);
        assert_eq!(p.ticks, 12);
        assert_eq!(p.threads, 3);
        assert_eq!(p.started, 9000);
        assert_eq!(p.rss_kb, 1000);
    }

    #[test]
    fn a_stat_cut_short_is_no_process() {
        assert_eq!(parse_stat("12 (x) S 1", 4), None);
        assert_eq!(parse_stat("nothing here", 4), None);
    }

    #[test]
    fn the_processors_are_read_together_and_each() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\ncpu1 40 0 30 400 30 0 0 0 0 0\nintr 5\n";
        let (all, cores) = processors(stat);
        assert_eq!(all, Ticks { total: 1000, idle: 850, user: 100, system: 50 });
        assert_eq!(cores.len(), 2);
        assert_eq!(cores[1].user, 40);
    }

    #[test]
    fn shares_are_of_the_time_between_two_readings() {
        let before = Ticks { total: 1000, idle: 850, user: 100, system: 50 };
        let now = Ticks { total: 1200, idle: 950, user: 170, system: 80 };
        // 200 ticks went by: 100 idle, 70 in programs, 30 in the kernel.
        assert_eq!(shares(before, now), (500, 350, 150));
        // Nothing to compare with, or no time gone by: nothing to say.
        assert_eq!(shares(Ticks::default(), now), (0, 0, 0));
        assert_eq!(shares(now, now), (0, 0, 0));
    }

    #[test]
    fn memory_is_read_in_kilobytes() {
        let info = "MemTotal:        995388 kB\nMemFree:          650908 kB\nMemAvailable:     642608 kB\nCached:           200000 kB\nSReclaimable:      10000 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n";
        let m = memory(info);
        assert_eq!(m.total, 995388);
        assert_eq!(m.available, 642608);
        assert_eq!(m.cached, 210000);
        assert_eq!(m.swap, 0);
    }

    #[test]
    fn memory_without_an_available_line_goes_by_free_and_cached() {
        let m = memory("MemTotal: 1000 kB\nMemFree: 300 kB\nCached: 200 kB\n");
        assert_eq!(m.available, 500);
    }

    #[test]
    fn accounts_are_named_by_number() {
        let names = users("root:x:0:0:root:/root:/bin/sh\ndisplay:x:100:100::/home/display:/bin/sh\nbad line\n");
        assert_eq!(names.get(&0).map(String::as_str), Some("root"));
        assert_eq!(names.get(&100).map(String::as_str), Some("display"));
        assert_eq!(names.len(), 2);
    }

    fn process(pid: i32, ppid: i32, program: &str) -> Process {
        Process {
            pid,
            ppid,
            program: program.to_string(),
            command: program.to_string(),
            ..Process::default()
        }
    }

    #[test]
    fn each_process_is_what_it_is_to_a_person() {
        let mut kernel = process(40, 2, "");
        kernel.command.clear();
        let all = vec![
            process(1, 0, "/init"),
            process(83, 1, "/sbin/saule-system"),
            process(95, 83, "/usr/sbin/bluetoothd"),
            process(102, 1, "/bin/session"),
            process(254, 1, "/bin/apps/files"),
            process(300, 1, "/bin/apps/terminal"),
            process(301, 300, "/bin/sh"),
            process(302, 301, "/usr/bin/top"),
            kernel,
        ];
        let kinds = kinds(&all, "/bin/apps");
        assert_eq!(kinds[&1], 's');
        assert_eq!(kinds[&83], 's');
        assert_eq!(kinds[&95], 's');
        assert_eq!(kinds[&102], 'w');
        assert_eq!(kinds[&254], 'a');
        assert_eq!(kinds[&300], 'a');
        assert_eq!(kinds[&301], 'p');
        assert_eq!(kinds[&302], 'p');
        assert_eq!(kinds[&40], 'k');
    }

    #[test]
    fn a_saule_program_is_the_file_saule_was_given_to_run() {
        let words = |text: &str| text.split(' ').map(str::to_string).collect::<Vec<_>>();
        let started = words("/bin/saule run /bin/apps/task-manager --all");
        let own = saule_program(&started).expect("a Saule program");
        assert_eq!(own, &words("/bin/apps/task-manager --all")[..]);
        // Called by its whole name, where the kernel keeps fifteen letters.
        assert_eq!(named("task-manager", own), "task-manager");
        assert_eq!(saule_program(&words("/bin/saule run")), None);
        assert_eq!(saule_program(&words("/bin/saule check src")), None);
        assert_eq!(saule_program(&words("/bin/sh run /bin/apps/files")), None);
        assert_eq!(saule_program(&words("/usr/bin/top")), None);
    }

    #[test]
    fn a_process_is_called_by_its_whole_name() {
        let words = |text: &str| text.split(' ').map(str::to_string).collect::<Vec<_>>();
        // The kernel keeps fifteen letters.
        assert_eq!(named("wpa_supplicant-", &words("/sbin/wpa_supplicant-long -i wlan0")), "wpa_supplicant-long");
        // One of busybox's tools, by the tool's name.
        assert_eq!(named("busybox", &words("/usr/bin/busybox udhcpc -i eth0")), "udhcpc");
        assert_eq!(named("busybox", &words("busybox")), "busybox");
        // A program that renamed itself keeps the name it took.
        assert_eq!(named("worker", &words("/usr/bin/server --fork")), "worker");
        // One of the kernel's own: nothing it was started as.
        assert_eq!(named("kworker/0:1", &[]), "kworker/0:1");
    }

    #[test]
    fn a_field_cannot_end_its_line() {
        assert_eq!(clean("a\tb\nc"), "a b c");
    }
}
