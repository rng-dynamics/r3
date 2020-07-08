#![feature(const_fn)]
#![feature(thread_local)]
#![feature(external_doc)]
#![feature(unsafe_block_in_unsafe_fn)] // `unsafe fn` doesn't imply `unsafe {}`
#![doc(include = "./lib.md")]
#![deny(unsafe_op_in_unsafe_fn)]
use atomic_ref::AtomicRef;
use constance::{
    kernel::{
        ClearInterruptLineError, EnableInterruptLineError, InterruptNum, InterruptPriority,
        PendInterruptLineError, Port, PortToKernel, QueryInterruptLineError,
        SetInterruptLinePriorityError, TaskCb, UTicks,
    },
    prelude::*,
    utils::intrusive_list::StaticListHead,
};
use once_cell::sync::OnceCell;
use parking_lot::{lock_api::RawMutex, Mutex};
use std::{cell::Cell, time::Instant};

#[cfg(unix)]
#[path = "threading_unix.rs"]
mod threading;

#[cfg(windows)]
#[path = "threading_win.rs"]
mod threading;

#[cfg(test)]
mod threading_test;

mod sched;
mod ums;
mod utils;

/// Used by `use_port!`
#[doc(hidden)]
pub extern crate constance;
/// Used by `use_port!`
#[doc(hidden)]
pub use std::sync::atomic::{AtomicBool, Ordering};
/// Used by `use_port!`
#[doc(hidden)]
pub extern crate env_logger;

/// The number of interrupt lines. The valid range of interrupt numbers is
/// defined as `0..NUM_INTERRUPT_LINES`
pub const NUM_INTERRUPT_LINES: usize = 1024;

/// The (software) interrupt line used for dispatching.
pub const INTERRUPT_LINE_DISPATCH: InterruptNum = 1023;

/// The default interrupt priority for [`INTERRUPT_LINE_DISPATCH`].
pub const INTERRUPT_PRIORITY_DISPATCH: InterruptPriority = 16384;

/// Implemented on a system type by [`use_port!`].
///
/// # Safety
///
/// Only meant to be implemented by [`use_port!`].
#[doc(hidden)]
pub unsafe trait PortInstance: Kernel + Port<PortTaskState = TaskState> {
    fn port_state() -> &'static State;
}

/// The internal state of the port.
///
/// # Safety
///
/// For the safety information of this type's methods, see the documentation of
/// the corresponding trait methods of `Port*`.
#[doc(hidden)]
pub struct State {
    thread_group: OnceCell<ums::ThreadGroup<sched::SchedState>>,
    join_handle: Mutex<Option<ums::ThreadGroupJoinHandle>>,
    origin: AtomicRef<'static, Instant>,
}

#[derive(Debug)]
pub struct TaskState {
    tsm: Mutex<Tsm>,
}

impl Init for TaskState {
    const INIT: Self = Self::new();
}

/// Task state machine
///
/// These don't exactly align with the task states defined in the kernel.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Tsm {
    /// The task's context state is not initialized. The kernel has to call
    /// `initialize_task_state` first before choosing this task as `running_task`.
    Uninit,
    /// The task's context state is initialized but hasn't started running.
    Dormant,
    /// The task is currently running.
    Running(ums::ThreadId),
}

/// The role of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreadRole {
    Unknown,
    /// The backing thread for an interrupt context.
    Interrupt,
    /// The backing thread for a task.
    Task,
}

thread_local! {
    /// The current thread's role. It's automatically assigned after the
    /// creation of a thread managed by the port.
    static THREAD_ROLE: Cell<ThreadRole> = Cell::new(ThreadRole::Unknown);
}

impl TaskState {
    pub const fn new() -> Self {
        Self {
            tsm: Mutex::const_new(RawMutex::INIT, Tsm::Uninit),
        }
    }

    fn assert_current_thread(&self) {
        // `self` must represent the current thread
        let expected_thread_id = match &*self.tsm.lock() {
            Tsm::Running(thread_id) => *thread_id,
            _ => unreachable!(),
        };
        assert_eq!(ums::current_thread(), Some(expected_thread_id));
    }

    unsafe fn exit_and_dispatch(&self, state: &'static State) -> ! {
        log::trace!("exit_and_dispatch({:p}) enter", self);
        self.assert_current_thread();

        let mut lock = state.thread_group.get().unwrap().lock();

        // Dissociate this thread from the task.
        let thread_id = match std::mem::replace(&mut *self.tsm.lock(), Tsm::Uninit) {
            Tsm::Running(thread_id) => thread_id,
            _ => unreachable!(),
        };

        // Make sure this thread will run to completion.
        //
        // Running all threads to completion is a prerequisite for a clean
        // shutdown. Shutdown will not complete if there are running threads.
        //
        // At this point, the thread is already dissociated from the task, so
        // the kernel will never choose this task again. However, the underlying
        // UMS thread is still alive. Thus, we need to temporarily override the
        // normal scheduling to ensure this thread will run to completion.
        lock.scheduler().recycle_thread(thread_id);
        lock.scheduler().cpu_lock = false;
        drop(lock);

        // Invoke the dispatcher
        unsafe { state.yield_cpu() };

        log::trace!("exit_and_dispatch({:p}) calling exit_thread", self);
        unsafe { ums::exit_thread() };
    }
}

#[allow(clippy::missing_safety_doc)]
impl State {
    pub const fn new() -> Self {
        Self {
            thread_group: OnceCell::new(),
            join_handle: Mutex::const_new(RawMutex::INIT, None),
            origin: AtomicRef::new(None),
        }
    }

    /// Initialize the user-mode scheduling system and boot the kernel.
    pub fn port_boot<System: Kernel>(&self) -> ! {
        // Create a UMS thread group.
        let (thread_group, join_handle) = ums::ThreadGroup::new(sched::SchedState::new::<System>());

        self.thread_group.set(thread_group).ok().unwrap();
        *self.join_handle.lock() = Some(join_handle);

        // Create the initial UMS worker thread, where the boot phase of the
        // kernel runs
        let mut lock = self.thread_group.get().unwrap().lock();
        let thread_id = lock.spawn(|_| {
            // Safety: We are a port, so it's okay to call this
            unsafe {
                <System as PortToKernel>::boot();
            }
        });
        log::trace!("startup thread = {:?}", thread_id);
        lock.scheduler().task_thread = Some(thread_id);
        lock.scheduler().recycle_thread(thread_id);
        lock.preempt();
        drop(lock);

        // Wait until the thread group shuts down
        let join_handle = self.join_handle.lock().take().unwrap();
        if let Err(e) = join_handle.join() {
            std::panic::resume_unwind(e);
        }

        panic!("the system has been shut down")
    }

    pub unsafe fn dispatch_first_task<System: PortInstance>(&'static self) -> !
    where
        // FIXME: Work-around for <https://github.com/rust-lang/rust/issues/43475>
        System::TaskReadyQueue: std::borrow::BorrowMut<[StaticListHead<TaskCb<System>>]>,
    {
        log::trace!("dispatch_first_task");
        assert!(self.is_cpu_lock_active());

        // Create a UMS worker thread for the dispatcher
        let mut lock = self.thread_group.get().unwrap().lock();

        // Configure PendSV
        lock.scheduler()
            .update_line(INTERRUPT_LINE_DISPATCH, |line| {
                line.priority = INTERRUPT_PRIORITY_DISPATCH;
                line.enable = true;
                line.pended = true;
                line.start = Some(Self::dispatch_handler::<System>);
            })
            .ok()
            .unwrap();

        lock.scheduler().cpu_lock = false;

        // Start scheduling
        assert!(sched::check_preemption_by_interrupt(
            self.thread_group.get().unwrap(),
            &mut lock
        ));
        drop(lock);

        // Safety: The requirement of `dispatch_first_task` explicitly allows
        // discarding the context.
        unsafe { ums::exit_thread() };
    }

    extern "C" fn dispatch_handler<System: PortInstance>()
    where
        // FIXME: Work-around for <https://github.com/rust-lang/rust/issues/43475>
        System::TaskReadyQueue: std::borrow::BorrowMut<[StaticListHead<TaskCb<System>>]>,
    {
        System::port_state().dispatch::<System>();
    }

    fn dispatch<System: PortInstance>(&'static self)
    where
        // FIXME: Work-around for <https://github.com/rust-lang/rust/issues/43475>
        System::TaskReadyQueue: std::borrow::BorrowMut<[StaticListHead<TaskCb<System>>]>,
    {
        unsafe { self.enter_cpu_lock() };
        unsafe { System::choose_running_task() };
        unsafe { self.leave_cpu_lock::<System>() };

        let mut lock = self.thread_group.get().unwrap().lock();

        // Tell the scheduler which task to run next
        lock.scheduler().task_thread = if let Some(task) = System::state().running_task() {
            log::trace!("dispatching task {:p}", task);

            let mut tsm = task.port_task_state.tsm.lock();

            match &*tsm {
                Tsm::Dormant => {
                    // Spawn a UMS worker thread for this task
                    let thread = lock.spawn(move |_| {
                        assert!(!self.is_cpu_lock_active());
                        log::debug!("task {:p} is now running", task);

                        THREAD_ROLE.with(|role| role.set(ThreadRole::Task));

                        // Safety: The port can call this
                        unsafe {
                            (task.attr.entry_point)(task.attr.entry_param);
                        }

                        // Safety: To my knowledge, we have nothing on the
                        // current thread' stack which are unsafe to
                        // `forget`. (`libstd`'s thread entry point might
                        // not be prepared to this, though...)
                        unsafe {
                            System::exit_task().unwrap();
                        }
                    });

                    log::trace!("spawned thread {:?} for the task {:p}", thread, task);

                    *tsm = Tsm::Running(thread);
                    Some(thread)
                }
                Tsm::Running(thread_id) => Some(*thread_id),
                Tsm::Uninit => unreachable!(),
            }
        } else {
            // Since we don't have timers or real interrupts yet, this means
            // we are in deadlock.
            //
            // The test code currently relies on this behavior (panic on
            // deadlock) to exit the dispatcher loop. When we have timers
            // and interrupts, we need to devise another way to exit the
            // dispatcher loop.
            panic!("No task to schedule");
        };
    }

    pub unsafe fn yield_cpu(&'static self) {
        log::trace!("yield_cpu");
        assert!(!self.is_cpu_lock_active());

        self.pend_interrupt_line(INTERRUPT_LINE_DISPATCH).unwrap();
    }

    pub unsafe fn exit_and_dispatch<System: Kernel>(
        &'static self,
        task: &'static TaskCb<System>,
    ) -> !
    where
        System: Port<PortTaskState = TaskState>,
        // Work-around <https://github.com/rust-lang/rust/issues/43475>
        System::TaskReadyQueue: std::borrow::BorrowMut<[StaticListHead<TaskCb<System>>]>,
    {
        log::trace!("exit_and_dispatch");
        assert!(self.is_cpu_lock_active());

        unsafe {
            task.port_task_state.exit_and_dispatch(self);
        }
    }

    pub unsafe fn enter_cpu_lock(&self) {
        log::trace!("enter_cpu_lock");

        let mut lock = self.thread_group.get().unwrap().lock();
        assert!(!lock.scheduler().cpu_lock);
        lock.scheduler().cpu_lock = true;
    }

    pub unsafe fn leave_cpu_lock<System: Kernel>(&'static self) {
        log::trace!("leave_cpu_lock");

        let mut lock = self.thread_group.get().unwrap().lock();
        assert!(lock.scheduler().cpu_lock);
        lock.scheduler().cpu_lock = false;

        if sched::check_preemption_by_interrupt(self.thread_group.get().unwrap(), &mut lock) {
            drop(lock);
            ums::yield_now();
        }
    }

    pub unsafe fn initialize_task_state<System: Kernel>(&self, task: &'static TaskCb<System>)
    where
        System: Port<PortTaskState = TaskState>,
    {
        log::trace!("initialize_task_state {:p}", task);

        let pts = &task.port_task_state;
        let mut tsm = pts.tsm.lock();
        match &*tsm {
            Tsm::Dormant => {}
            Tsm::Running(_) => {
                todo!("terminating a thread is not implemented yet");
            }
            Tsm::Uninit => {
                *tsm = Tsm::Dormant;
            }
        }
    }

    pub fn is_cpu_lock_active(&self) -> bool {
        (self.thread_group.get().unwrap().lock())
            .scheduler()
            .cpu_lock
    }

    pub fn is_interrupt_context(&self) -> bool {
        THREAD_ROLE.with(|role| match role.get() {
            ThreadRole::Interrupt => true,
            ThreadRole::Task => false,
            _ => panic!("`is_interrupt_context` was called from an unknown thread"),
        })
    }

    pub fn set_interrupt_line_priority<System: Kernel>(
        &'static self,
        num: InterruptNum,
        priority: InterruptPriority,
    ) -> Result<(), SetInterruptLinePriorityError> {
        log::trace!("set_interrupt_line_priority{:?}", (num, priority));

        let mut lock = self.thread_group.get().unwrap().lock();
        lock.scheduler()
            .update_line(num, |line| line.priority = priority)
            .map_err(|sched::BadIntLineError| SetInterruptLinePriorityError::BadParam)?;

        if sched::check_preemption_by_interrupt(self.thread_group.get().unwrap(), &mut lock) {
            drop(lock);
            ums::yield_now();
        }

        Ok(())
    }

    pub fn enable_interrupt_line<System: Kernel>(
        &'static self,
        num: InterruptNum,
    ) -> Result<(), EnableInterruptLineError> {
        log::trace!("enable_interrupt_line{:?}", (num,));

        let mut lock = self.thread_group.get().unwrap().lock();
        lock.scheduler()
            .update_line(num, |line| line.enable = true)
            .map_err(|sched::BadIntLineError| EnableInterruptLineError::BadParam)?;

        if sched::check_preemption_by_interrupt(self.thread_group.get().unwrap(), &mut lock) {
            drop(lock);
            ums::yield_now();
        }

        Ok(())
    }

    pub fn disable_interrupt_line(
        &self,
        num: InterruptNum,
    ) -> Result<(), EnableInterruptLineError> {
        log::trace!("disable_interrupt_line{:?}", (num,));

        (self.thread_group.get().unwrap().lock())
            .scheduler()
            .update_line(num, |line| line.enable = false)
            .map_err(|sched::BadIntLineError| EnableInterruptLineError::BadParam)
    }

    pub fn pend_interrupt_line(
        &'static self,
        num: InterruptNum,
    ) -> Result<(), PendInterruptLineError> {
        log::trace!("pend_interrupt_line{:?}", (num,));

        let mut lock = self.thread_group.get().unwrap().lock();
        lock.scheduler()
            .update_line(num, |line| line.pended = true)
            .map_err(|sched::BadIntLineError| PendInterruptLineError::BadParam)?;

        if sched::check_preemption_by_interrupt(self.thread_group.get().unwrap(), &mut lock) {
            drop(lock);
            ums::yield_now();
        }

        Ok(())
    }

    pub fn clear_interrupt_line(&self, num: InterruptNum) -> Result<(), ClearInterruptLineError> {
        log::trace!("clear_interrupt_line{:?}", (num,));

        (self.thread_group.get().unwrap().lock())
            .scheduler()
            .update_line(num, |line| line.pended = false)
            .map_err(|sched::BadIntLineError| ClearInterruptLineError::BadParam)
    }

    pub fn is_interrupt_line_pending(
        &self,
        num: InterruptNum,
    ) -> Result<bool, QueryInterruptLineError> {
        (self.thread_group.get().unwrap().lock())
            .scheduler()
            .is_line_pended(num)
            .map_err(|sched::BadIntLineError| QueryInterruptLineError::BadParam)
    }

    // TODO: Make these customizable to test the kernel under multiple conditions
    pub const MAX_TICK_COUNT: UTicks = UTicks::MAX;
    pub const MAX_TIMEOUT: UTicks = UTicks::MAX / 2;

    pub fn tick_count(&self) -> UTicks {
        let origin = if let Some(x) = self.origin.load(Ordering::Acquire) {
            x
        } else {
            // Establish an origin point.
            let origin = Box::leak(Box::new(Instant::now()));

            // Store `origin` to `self.origin`.
            //
            // 1. If `self.origin` is already initialized at this point, discard
            //    `origin`. Use `Acquire` to synchronize with the canonical
            //    initializing thread.
            //
            // 2. Otherwise, `origin` is now the canonical origin. Use `Release`
            //    to synchronize with other threads, ensuring the initialized
            //    contents of `origin` is visible to them.
            //
            //    `compare_exchange` requires that the success ordering is
            //    stronger than the failure ordering, so we actually have to use
            //    `AcqRel` here.
            //
            // (Actually, this really doesn't matter because it's a kernel for
            // a uniprocessor system, anyway.)
            match self.origin.compare_exchange(
                None,
                Some(origin),
                Ordering::AcqRel,  // case 2
                Ordering::Acquire, // case 1
            ) {
                Ok(_) => origin,      // case 2
                Err(x) => x.unwrap(), // case 1
            }
        };

        let micros = Instant::now().duration_since(*origin).as_micros();

        /// Implementation of <https://xkcd.com/221/> with a different magic
        /// number
        fn get_random_number() -> UTicks {
            0x00c0ffee
        }

        // Calculate `micros % MAX_TICK_COUNT + 1` by truncating upper bits. Add
        // some random number so that the kernel doesn't depend on zero-start.
        (micros as UTicks).wrapping_add(get_random_number())
    }

    pub fn pend_tick_after(&self, tick_count_delta: UTicks) {
        // TODO
    }

    pub fn pend_tick(&self) {
        // TODO
    }
}

#[macro_export]
macro_rules! use_port {
    (unsafe $vis:vis struct $sys:ident) => {
        $vis struct $sys;

        mod port_std_impl {
            use super::$sys;
            use $crate::constance::kernel::{
                ClearInterruptLineError, EnableInterruptLineError, InterruptNum, InterruptPriority,
                PendInterruptLineError, Port, QueryInterruptLineError, SetInterruptLinePriorityError,
                TaskCb, PortToKernel, PortInterrupts, PortThreading, UTicks, PortTimer,
            };
            use $crate::{State, TaskState, PortInstance};

            pub(super) static PORT_STATE: State = State::new();

            unsafe impl PortInstance for $sys {
                #[inline]
                fn port_state() -> &'static State {
                    &PORT_STATE
                }
            }

            // Assume `$sys: Kernel`
            unsafe impl PortThreading for $sys {
                type PortTaskState = TaskState;
                const PORT_TASK_STATE_INIT: Self::PortTaskState = TaskState::new();

                unsafe fn dispatch_first_task() -> ! {
                    PORT_STATE.dispatch_first_task::<Self>()
                }

                unsafe fn yield_cpu() {
                    PORT_STATE.yield_cpu()
                }

                unsafe fn exit_and_dispatch(task: &'static TaskCb<Self>) -> ! {
                    PORT_STATE.exit_and_dispatch::<Self>(task);
                }

                unsafe fn enter_cpu_lock() {
                    PORT_STATE.enter_cpu_lock()
                }

                unsafe fn leave_cpu_lock() {
                    PORT_STATE.leave_cpu_lock::<Self>()
                }

                unsafe fn initialize_task_state(task: &'static TaskCb<Self>) {
                    PORT_STATE.initialize_task_state(task)
                }

                fn is_cpu_lock_active() -> bool {
                    PORT_STATE.is_cpu_lock_active()
                }

                fn is_interrupt_context() -> bool {
                    PORT_STATE.is_interrupt_context()
                }
            }

            unsafe impl PortInterrupts for $sys {
                const MANAGED_INTERRUPT_PRIORITY_RANGE:
                    ::std::ops::Range<InterruptPriority> = 0..InterruptPriority::max_value();

                unsafe fn set_interrupt_line_priority(
                    line: InterruptNum,
                    priority: InterruptPriority,
                ) -> Result<(), SetInterruptLinePriorityError> {
                    PORT_STATE.set_interrupt_line_priority::<Self>(line, priority)
                }

                unsafe fn enable_interrupt_line(line: InterruptNum) -> Result<(), EnableInterruptLineError> {
                    PORT_STATE.enable_interrupt_line::<Self>(line)
                }

                unsafe fn disable_interrupt_line(line: InterruptNum) -> Result<(), EnableInterruptLineError> {
                    PORT_STATE.disable_interrupt_line(line)
                }

                unsafe fn pend_interrupt_line(line: InterruptNum) -> Result<(), PendInterruptLineError> {
                    PORT_STATE.pend_interrupt_line(line)
                }

                unsafe fn clear_interrupt_line(line: InterruptNum) -> Result<(), ClearInterruptLineError> {
                    PORT_STATE.clear_interrupt_line(line)
                }

                unsafe fn is_interrupt_line_pending(
                    line: InterruptNum,
                ) -> Result<bool, QueryInterruptLineError> {
                    PORT_STATE.is_interrupt_line_pending(line)
                }
            }

            impl PortTimer for $sys {
                const MAX_TICK_COUNT: UTicks = State::MAX_TICK_COUNT;
                const MAX_TIMEOUT: UTicks = State::MAX_TIMEOUT;

                unsafe fn tick_count() -> UTicks {
                    PORT_STATE.tick_count()
                }

                unsafe fn pend_tick_after(tick_count_delta: UTicks) {
                    PORT_STATE.pend_tick_after(tick_count_delta)
                }

                unsafe fn pend_tick() {
                    PORT_STATE.pend_tick()
                }
            }
        }

        fn main() {
            use $crate::constance::kernel::PortToKernel;

            $crate::env_logger::init();

            port_std_impl::PORT_STATE.port_boot::<$sys>();
        }
    };
}
