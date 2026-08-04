//! Fixed-size work-stealing pool for `thread::spawn` / auto-par fork-join.
//!
//! OS threads are created once per root VM (see [`crate::thread::WorkerCap`]
//! pool size). Jobs are pushed to a shared injector and stolen via
//! [`crossbeam_deque`]; `join` help-steals so fork-join does not deadlock
//! when workers sit on joins.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

use crossbeam_deque::{Injector, Steal, Stealer, Worker};

use crate::ffi::Natives;
use crate::thread::{
    HostStateGuard, JoinState, LiveThreadRegistry, PortableValue, SharedPrintWriter, SpawnArg,
    ThreadErrorTag, ThreadProgram, ThreadSpawnContext, WORKER_STACK_SLOTS, spawn_arg_to_value,
    value_to_portable,
};
use crate::vm::Machine;

/// One unit of work for the reactor (isolated `call_function` on a worker VM).
pub struct Job {
    pub entry: u32,
    pub args: Vec<SpawnArg>,
    pub state: Arc<JoinState>,
    pub program: Arc<ThreadProgram>,
    pub natives: Natives,
    pub shared_print: Option<Arc<Mutex<Vec<u8>>>>,
    pub live_threads: LiveThreadRegistry,
    pub reactor: Arc<Reactor>,
}

/// Per-root-VM work-stealing reactor.
pub struct Reactor {
    injector: Injector<Job>,
    stealers: RwLock<Vec<Stealer<Job>>>,
    sleep: Mutex<()>,
    sleep_cvar: Condvar,
    n_workers: usize,
    started: OnceLock<()>,
    inflight: AtomicUsize,
    shutdown: AtomicBool,
}

impl Reactor {
    pub fn new(n_workers: usize) -> Arc<Self> {
        Arc::new(Self {
            injector: Injector::new(),
            stealers: RwLock::new(Vec::with_capacity(n_workers)),
            sleep: Mutex::new(()),
            sleep_cvar: Condvar::new(),
            n_workers: n_workers.max(1),
            started: OnceLock::new(),
            inflight: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        })
    }

    pub fn worker_count(&self) -> usize {
        self.n_workers
    }

    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    fn ensure_started(self: &Arc<Self>) {
        let reactor = Arc::clone(self);
        let _ = self.started.get_or_init(|| {
            for i in 0..reactor.n_workers {
                let r = Arc::clone(&reactor);
                let name = format!("coil-reactor-{i}");
                thread::Builder::new()
                    .name(name)
                    // Nested join-help can be deep for recursive auto-par.
                    .stack_size(8 * 1024 * 1024)
                    .spawn(move || worker_loop(r))
                    .expect("coil reactor worker");
            }
        });
    }

    fn notify(&self) {
        self.sleep_cvar.notify_one();
    }

    /// Submit `job` to the pool (starts workers lazily).
    pub fn submit(self: &Arc<Self>, job: Job) {
        self.ensure_started();
        self.inflight.fetch_add(1, Ordering::SeqCst);
        match try_push_local(job) {
            Ok(()) => {}
            Err(job) => self.injector.push(job),
        }
        self.notify();
    }

    fn register_stealer(&self, stealer: Stealer<Job>) {
        self.stealers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(stealer);
    }

    fn find_job(&self, local: &Worker<Job>) -> Option<Job> {
        if let Some(job) = local.pop() {
            return Some(job);
        }
        steal_from_injector(&self.injector).or_else(|| steal_from_peers(&self.stealers))
    }

    fn steal_job(&self) -> Option<Job> {
        steal_from_injector(&self.injector).or_else(|| steal_from_peers(&self.stealers))
    }

    /// Block until `state` completes, helping run stolen jobs meanwhile.
    pub fn wait_join(self: &Arc<Self>, state: &JoinState) -> Result<PortableValue, ThreadErrorTag> {
        if is_pool_worker() {
            return wait_join_on_worker(self, state);
        }
        self.wait_join_with_helper_vm(state)
    }

    fn wait_join_with_helper_vm(
        self: &Arc<Self>,
        state: &JoinState,
    ) -> Result<PortableValue, ThreadErrorTag> {
        loop {
            if let Some(r) = state.try_take_result() {
                return r;
            }
            if let Some(job) = self.steal_job() {
                let mut vm = Box::new(Machine::<WORKER_STACK_SLOTS>::default());
                run_job_on_vm(&mut vm, job);
                continue;
            }
            {
                let mut g = state.inner_lock();
                if g.result.is_some() {
                    return g
                        .result
                        .take()
                        .unwrap_or(Err(ThreadErrorTag::JoinFailed));
                }
                let wait = state
                    .finished_cvar()
                    .wait_timeout(g, Duration::from_millis(1));
                match wait {
                    Ok((guard, _)) => drop(guard),
                    Err(poisoned) => drop(poisoned.into_inner().0),
                }
            }
        }
    }
}

fn steal_from_injector(injector: &Injector<Job>) -> Option<Job> {
    loop {
        match injector.steal() {
            Steal::Success(job) => return Some(job),
            Steal::Empty => return None,
            Steal::Retry => continue,
        }
    }
}

fn steal_from_peers(stealers: &RwLock<Vec<Stealer<Job>>>) -> Option<Job> {
    let guard = stealers.read().unwrap_or_else(|e| e.into_inner());
    let n = guard.len();
    if n == 0 {
        return None;
    }
    let start = steal_cursor().fetch_add(1, Ordering::Relaxed) % n;
    for i in 0..n {
        let s = &guard[(start + i) % n];
        loop {
            match s.steal() {
                Steal::Success(job) => return Some(job),
                Steal::Empty => break,
                Steal::Retry => continue,
            }
        }
    }
    None
}

fn steal_cursor() -> &'static AtomicUsize {
    static CURSOR: AtomicUsize = AtomicUsize::new(0);
    &CURSOR
}

thread_local! {
    static LOCAL_WORKER: std::cell::RefCell<Option<Worker<Job>>> =
        const { std::cell::RefCell::new(None) };
    static IS_POOL_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn is_pool_worker() -> bool {
    IS_POOL_WORKER.with(|c| c.get())
}

fn try_push_local(job: Job) -> Result<(), Job> {
    LOCAL_WORKER.with(|slot| {
        if let Some(w) = slot.borrow_mut().as_mut() {
            w.push(job);
            Ok(())
        } else {
            Err(job)
        }
    })
}

fn worker_loop(reactor: Arc<Reactor>) {
    let local = Worker::new_fifo();
    reactor.register_stealer(local.stealer());

    let mut vm = Machine::<WORKER_STACK_SLOTS>::default();
    vm.set_reactor(Arc::clone(&reactor));

    IS_POOL_WORKER.with(|c| c.set(true));
    LOCAL_WORKER.with(|slot| *slot.borrow_mut() = Some(local));

    loop {
        if reactor.shutdown.load(Ordering::Relaxed) {
            break;
        }
        let job = LOCAL_WORKER.with(|slot| {
            let w = slot.borrow();
            let local_ref = w.as_ref().expect("pool worker local queue");
            reactor.find_job(local_ref)
        });
        match job {
            Some(job) => run_job_on_vm(&mut vm, job),
            None => {
                let g = reactor.sleep.lock().unwrap_or_else(|e| e.into_inner());
                let _ = reactor
                    .sleep_cvar
                    .wait_timeout(g, Duration::from_millis(2));
            }
        }
    }

    LOCAL_WORKER.with(|slot| *slot.borrow_mut() = None);
    IS_POOL_WORKER.with(|c| c.set(false));
}

fn wait_join_on_worker(
    reactor: &Arc<Reactor>,
    state: &JoinState,
) -> Result<PortableValue, ThreadErrorTag> {
    loop {
        if let Some(r) = state.try_take_result() {
            return r;
        }
        let job = LOCAL_WORKER.with(|slot| {
            let w = slot.borrow();
            let local_ref = w.as_ref()?;
            reactor.find_job(local_ref)
        });
        if let Some(job) = job {
            // Heap-allocate the help VM so nested join-help does not blow the
            // OS stack with stacked `Machine` values.
            let mut vm = Box::new(Machine::<WORKER_STACK_SLOTS>::default());
            run_job_on_vm(&mut vm, job);
            continue;
        }
        {
            let mut g = state.inner_lock();
            if g.result.is_some() {
                return g
                    .result
                    .take()
                    .unwrap_or(Err(ThreadErrorTag::JoinFailed));
            }
            let wait = state
                .finished_cvar()
                .wait_timeout(g, Duration::from_millis(1));
            match wait {
                Ok((guard, _)) => drop(guard),
                Err(poisoned) => drop(poisoned.into_inner().0),
            }
        }
    }
}

fn run_job_on_vm(vm: &mut Machine<WORKER_STACK_SLOTS>, job: Job) {
    let Job {
        entry,
        args,
        state,
        program,
        natives,
        shared_print,
        live_threads,
        reactor,
    } = job;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.install_natives(&natives);
        vm.set_thread_program(Arc::clone(&program));
        vm.set_program_debug(program.debug.clone());
        vm.set_live_threads(Arc::clone(&live_threads));
        vm.set_reactor(Arc::clone(&reactor));
        vm.set_worker_cap(crate::thread::WorkerCap::from_count(reactor.worker_count()));
        if let Some(buf) = &shared_print {
            vm.set_shared_print(Arc::clone(buf));
            vm.with_output(SharedPrintWriter(Arc::clone(buf)));
            crate::io::set_shared_print_redirect(Some(Arc::clone(buf)));
        }
        vm.load_program(
            program.code.as_slice(),
            program.constants.as_slice(),
            program.strings.as_slice(),
        );
        vm.init_static_slots(program.static_slot_count);

        let _guard = HostStateGuard::enter(vm);
        let mut child_args = Vec::with_capacity(args.len());
        for a in args {
            child_args.push(spawn_arg_to_value(vm.heap_mut(), a)?);
        }
        let ret = vm.call_function(entry, &child_args);
        if vm.panicked() {
            return Err(ThreadErrorTag::JoinFailed);
        }
        value_to_portable(vm.heap(), ret)
    }));

    let stored = match result {
        Ok(Ok(pv)) => Ok(pv),
        Ok(Err(tag)) => Err(tag),
        Err(_) => Err(ThreadErrorTag::JoinFailed),
    };
    state.store_result(stored);
    reactor.inflight.fetch_sub(1, Ordering::SeqCst);
    reactor.notify();
}

/// Build a [`Job`] from spawn context + decoded args.
pub fn job_from_spawn_context(
    ctx: ThreadSpawnContext,
    entry: u32,
    args: Vec<SpawnArg>,
    state: Arc<JoinState>,
) -> Job {
    Job {
        entry,
        args,
        state,
        program: ctx.program,
        natives: ctx.natives,
        shared_print: ctx.shared_print,
        live_threads: ctx.live_threads,
        reactor: ctx.reactor,
    }
}
