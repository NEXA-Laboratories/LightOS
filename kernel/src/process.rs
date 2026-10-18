//! Process table, round-robin scheduler and built-in demo programs.
//!
//! 0.1.0 runs kernel-mode tasks cooperatively: a task is a `fn()` that the
//! scheduler runs to completion. Preemptive ring-3 processes arrive with the
//! ELF loader in 0.2.0.
use alloc::{string::{String, ToString}, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts::{self, without_interrupts};

pub const TICK_HZ: u64 = 100;
pub const MAX_PROCESSES: usize = 1024;

static TICKS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Blocked,
    Terminated,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Ready => "Ready",
            State::Running => "Running",
            State::Blocked => "Blocked",
            State::Terminated => "Terminated",
        }
    }
}

#[derive(Clone)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub state: State,
    pub priority: u8,
    pub ticks: u64,
    pub exit_code: i32,
    entry: Option<fn()>,
}

struct Manager {
    procs: Vec<Process>,
    next_pid: u32,
    current: Option<u32>,
    cursor: usize,
    switches: u64,
    ext_prev: Option<u32>,
}

static MANAGER: Mutex<Manager> = Mutex::new(Manager {
    procs: Vec::new(),
    next_pid: 1,
    current: None,
    cursor: 0,
    switches: 0,
    ext_prev: None,
});

impl Manager {
    fn find(&mut self, pid: u32) -> Option<&mut Process> {
        self.procs.iter_mut().find(|p| p.pid == pid)
    }

    fn add(&mut self, name: &str, priority: u8, state: State, entry: Option<fn()>) -> Result<u32, &'static str> {
        if self.procs.len() >= MAX_PROCESSES {
            self.procs.retain(|p| p.state != State::Terminated);
            if self.procs.len() >= MAX_PROCESSES {
                return Err("process table full");
            }
        }
        let pid = self.next_pid;
        self.next_pid += 1;
        self.procs.push(Process {
            pid,
            name: name.to_string(),
            state,
            priority,
            ticks: 0,
            exit_code: 0,
            entry,
        });
        Ok(pid)
    }

    /// Round-robin: next Ready process with an entry point after the cursor.
    fn pick_next(&mut self) -> Option<usize> {
        let n = self.procs.len();
        for i in 1..=n {
            let idx = (self.cursor + i) % n;
            if self.procs[idx].state == State::Ready && self.procs[idx].entry.is_some() {
                self.cursor = idx;
                return Some(idx);
            }
        }
        None
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Timer IRQ hook. Must not allocate or block.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    if let Some(mut m) = MANAGER.try_lock() {
        if let Some(pid) = m.current {
            if let Some(p) = m.find(pid) {
                p.ticks += 1;
            }
        }
    }
}

pub fn sleep_ms(ms: u64) {
    let target = ticks() + ms * TICK_HZ / 1000 + 1;
    while ticks() < target {
        interrupts::enable_and_hlt();
    }
}

pub fn init() {
    without_interrupts(|| {
        let mut m = MANAGER.lock();
        m.add("init", 5, State::Ready, None).unwrap();
        let shell = m.add("los-shell", 5, State::Running, None).unwrap();
        m.current = Some(shell);
    });
}

pub fn spawn(name: &str, priority: u8, entry: fn()) -> Result<u32, &'static str> {
    without_interrupts(|| MANAGER.lock().add(name, priority, State::Ready, Some(entry)))
}

/// Run one Ready process to completion. Returns its PID.
pub fn step() -> Option<u32> {
    let (pid, entry, prev) = without_interrupts(|| {
        let mut m = MANAGER.lock();
        let idx = m.pick_next()?;
        let prev = m.current;
        if let Some(p) = prev {
            if let Some(proc_) = m.find(p) {
                proc_.state = State::Ready;
            }
        }
        m.procs[idx].state = State::Running;
        let pid = m.procs[idx].pid;
        let entry = m.procs[idx].entry?;
        m.current = Some(pid);
        m.switches += 1;
        Some((pid, entry, prev))
    })?;

    entry();

    without_interrupts(|| {
        let mut m = MANAGER.lock();
        if let Some(p) = m.find(pid) {
            p.state = State::Terminated;
        }
        if let Some(prev) = prev {
            if let Some(p) = m.find(prev) {
                p.state = State::Running;
            }
        }
        m.current = prev;
    });
    Some(pid)
}

/// Run every Ready process (round-robin order). Returns how many ran.
pub fn run_all() -> usize {
    let mut n = 0;
    while step().is_some() {
        n += 1;
    }
    n
}

pub fn kill(pid: u32) -> Result<(), &'static str> {
    if pid <= 2 {
        return Err("refusing to kill a system process");
    }
    without_interrupts(|| {
        let mut m = MANAGER.lock();
        match m.find(pid) {
            Some(p) if p.state != State::Terminated => {
                p.state = State::Terminated;
                Ok(())
            }
            Some(_) => Err("process already terminated"),
            None => Err("no such process"),
        }
    })
}

pub fn reap() -> usize {
    without_interrupts(|| {
        let mut m = MANAGER.lock();
        let before = m.procs.len();
        m.procs.retain(|p| p.state != State::Terminated);
        before - m.procs.len()
    })
}

pub fn current_pid() -> u32 {
    without_interrupts(|| MANAGER.lock().current.unwrap_or(0))
}

/// sys_exit: mark the current process terminated with `code`.
pub fn terminate_current(code: i32) {
    without_interrupts(|| {
        let mut m = MANAGER.lock();
        if let Some(pid) = m.current {
            if pid > 2 {
                if let Some(p) = m.find(pid) {
                    p.exit_code = code;
                    p.state = State::Terminated;
                }
            }
        }
    });
}

/// Register a process for a loaded ELF app and make it the current one.
pub fn begin_external(name: &str) -> u32 {
    without_interrupts(|| {
        let mut m = MANAGER.lock();
        let pid = m.add(name, 5, State::Running, None).unwrap_or(0);
        let prev = m.current;
        if let Some(p) = prev {
            if let Some(proc_) = m.find(p) {
                proc_.state = State::Ready;
            }
        }
        m.ext_prev = prev;
        m.current = Some(pid);
        m.switches += 1;
        pid
    })
}

pub fn end_external(pid: u32, code: i32) {
    without_interrupts(|| {
        let mut m = MANAGER.lock();
        if let Some(p) = m.find(pid) {
            p.state = State::Terminated;
            p.exit_code = code;
        }
        let prev = m.ext_prev.take();
        if let Some(prev) = prev {
            if let Some(p) = m.find(prev) {
                p.state = State::Running;
            }
        }
        m.current = prev;
    });
}

pub fn snapshot() -> Vec<Process> {
    without_interrupts(|| MANAGER.lock().procs.clone())
}

pub fn switches() -> u64 {
    without_interrupts(|| MANAGER.lock().switches)
}

// ---------------------------------------------------------------- demo programs

pub const PROGRAMS: &[(&str, fn())] = &[
    ("hello", prog_hello),
    ("counter", prog_counter),
    ("sysdemo", prog_sysdemo),
];

fn prog_hello() {
    println!("Hello from LightOS process {}!", crate::syscall::getpid());
}

fn prog_counter() {
    let pid = crate::syscall::getpid();
    for i in 1..=5 {
        println!("[pid {}] count {}", pid, i);
        sleep_ms(100);
    }
}

fn prog_sysdemo() {
    use crate::syscall as sc;
    sc::write(1, "sysdemo: write via int 0x80\n");
    println!("sysdemo: PID = {}", sc::getpid());
    println!("sysdemo: uptime = {} ticks", sc::uptime());
    sc::yield_now();
    println!("sysdemo: done");
}
