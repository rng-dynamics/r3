//! Timers
use core::{fmt, hash, marker::PhantomData, mem::ManuallyDrop};

use super::{
    timeout,
    utils::{assume_cpu_lock, lock_cpu, CpuLockCell, CpuLockGuard, CpuLockTokenRefMut},
    BadIdError, Id, Kernel, SetTimerDelayError, SetTimerPeriodError, StartTimerError,
    StopTimerError,
};
use crate::{
    time::Duration,
    utils::{pin::static_pin, Init},
};

#[cfg_attr(doc, svgbobdoc::transform)]
/// Represents a single timer in a system.
///
/// This type is ABI-compatible with [`Id`].
///
/// <div class="admonition-follows"></div>
///
/// > **Relation to Other Specifications:** A similar concept exists in almost
/// > every operating system.
///
/// <div class="toc-header"></div>
///
///  - [Timer States](#timer-states)
///  - [Timer Scheduling](#timer-scheduling)
///      - [Overdue Timers](#overdue-timers)
///      - [Start/Stop](#startstop)
///      - [Dynamic Period](#dynamic-period)
///      - [Infinite Delay and/or Period](#infinite-delay-andor-period)
///  - [Examples](#examples)
///      - [Periodic Timer](#periodic-timer)
///      - [One-Shot Timer](#one-shot-timer)
///  - [Methods](#implementations)  <!-- this section is generated by rustdoc -->
///
/// # Timer States
///
/// A timer may be in one of the following states:
///
///  - **Dormant** — The timer is not running and can be [started].
///
///  - **Active** — The timer is running and can be [stopped].
///
/// <center>
/// ```svgbob
/// ,---------------,             start              ,--------------,
/// |               | -----------------------------> |              |
/// |    Dormant    |                                |    Active    |
/// |               | <----------------------------- |              |
/// '---------------'              stop              '--------------'
/// ```
/// </center>
///
/// [started]: Timer::start
/// [stopped]: Timer::stop
///
/// # Timer Scheduling
///
/// The scheduling of a timer is determined by two state variables:
///
///  - The [delay] is an optional non-negative [duration] value
///    (`Option<Duration>`) that specifies the minimum period of time before the
///    callback function gets called.
///
///    If the delay is `None`, it's treated as infinity and the function will
///    never execute.
///
///    While a timer is active, this value decreases at a steady rate. If the
///    system can't process a timer for an extended period of time, this value
///    might temporarily fall negative.
///
///  - The [period] is an optional non-negative duration value. On expiration,
///    the system adds this value to the timer's delay.
///
/// [delay]: Timer::set_delay
/// [period]: Timer::set_period
/// [duration]: crate::time::Duration
///
/// ## Overdue Timers
///
/// <center>
/// ```svgbob
/// ​
/// Higher-priority interrupt               __________
/// or CPU Lock                            |__________|
///
///                               _____                _____ _____    _____
/// Timer callback               |_____|              |_____|_____|  |_____|
///                              1                    2     3        4
///
/// Delay     7  6  5  4  3  2  1  4  3  2  1  0 -1 -2  1  0  3  2  1  4  3  2  1
///         ├──┬──┬──┬──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┤
///         ↑    initial delay   1   period  2   period  3   period  4   period
///     activated
/// ​
/// ```
/// </center>
///
/// When scheduling a next tick, the system takes the observed timer handling
/// latency into account and makes the new delay shorter than the period as
/// needed to ensure that the callback function is called in a steady rate. This
/// behavior is illustrated by the above figure. This is accomplished by adding
/// the specified period to the timer's absolute arrival time instead of
/// recalculating the arrival time based on the current system time. The delay
/// is a difference between the current system time and the arrival time.
///
/// Note that the system does not impose any limit on the extent of this
/// behavior. To put this simply, *if one second elapses, the system makes one
/// second worth of calls no matter what.*
/// If a periodic timer's callback function couldn't complete within the
/// timer's period, the timer latency would steadily increase until it reaches
/// the point where various internal assumptions (such as
/// [`TIME_HARD_HEADROOM`]) get broken. While the system is processing overdue
/// calls, the timer interrupt handler will not return. Some port timer drivers
/// (most notably the Arm-M tickful SysTick driver) have much lower tolerance
/// for this.
/// To avoid this catastrophic situation, an application should take the
/// precautions shown below:
///
///  - Don't perform an operation that might take an unbounded time in a timer
///    callback function.
///
///  - Off-load time-consuming operations to a task, which is [activated] or
///    [unparked] by a timer callback function.
///
///  - Don't specify zero as period unless you know what you are doing.
///
///  - Keep your target platform's performance characteristics in your mind.
///
/// [`TIME_HARD_HEADROOM`]: crate::kernel::TIME_HARD_HEADROOM
/// [activated]: crate::kernel::Task::activate
/// [unparked]: crate::kernel::Task::unpark
///
/// ## Start/Stop
///
/// When a timer is [stopped], the timer will not fire anymore and the delay
/// remains stationary at the captured value. If the captured value is negative,
/// it's rounded to zero. This means that if there are more than one outstanding
/// call at the moment of stopping, they will be dropped.
///
/// <center>
/// ```svgbob
///                   _____       _____                   _____       _____
/// Timer callback   |_____|     |_____|                 |_____|     |_____|
///                  1           2                       3           4
///
///                  ├──┬──┬──┬──┼──┤╴╴╴╴╴╴╴╴╴╴╴├──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┤
///                  1           2  ↑           ↑        3           4
///                               stop        start
///
///                   _____ _____ _____ _____         _____ _____ _____
/// Timer callback   |_____|_____|_____|_____|       |_____|_____|_____|
///                  1     2     3     4             5     6     7
///
///                  ├──┼──┼──┼──┼──┼──┼─┤╴╴╴╴╴╴╴╴╴╴╴├──┼──┼──┼──┼──┼──┤
///                  1  2  3  4  x  x  x ↑           ↑5 6  7  8  9  10
///                                     stop       start
/// ​
/// ```
/// </center>
///
/// Another way to stop a timer is to [set the delay or the period to `None`
/// (infinity)](#infinite-delay-andor-period).
///
/// [stopped]: Timer::stop
///
/// ## Dynamic Period
///
/// The period can be changed anytime. The system reads it before calling a
/// timer callback function and adds it to the timer's current delay value.
///
/// <center>
/// ```svgbob
///                   _____       _____       _____    _____    _____
/// Timer callback   |_____|     |_____|     |_____|  |_____|  |_____|
///                  1           2           3        4        5
///
/// Delay             4  3  2  1  4  3  2  1  3  2  1  3  2  1  3  2  1
///                  ├──┬──┬──┬──┼──┬──┬──┬──┤
///                  1           2  ↑
///              period = 4     period ← 3   ├──┬──┬──┼──┬──┬──┼──┬──┬──┤
///                                          3        4        5
///
///                   _____ _____ _____ _____ _____ _____ _____       _____
/// Timer callback   |_____|_____|_____|_____|_____|_____|_____|     |_____|
///                  1     2     3     4     5     6     7           8
///
/// Delay             1  0  0  -1 -1 -2 -2 -3 0  -1 2  1  4  3  2  1  4
///                  ├──┼──┼──┼──┼──┼──┼┤
///                  1  2  3  4  x  x  x↑
///              period = 1      ├──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┼──┤
///                              5      ↑    6           7           8
///                                period ← 4
/// ​
/// ```
/// </center>
///
/// It might be tricky to understand the outcome of changing the period when
/// there are overdue calls. It could be explained in this way: *If there are
/// one second worth of calls pending, there will still be one second worth of
/// calls pending after changing the period.*
///
/// ## Infinite Delay and/or Period
///
/// If [`delay` is set] to `None` (infinity), the timer will stop firing. Note
/// that the timer is still in the Active state, and the correct way to restart
/// this timer is to reset the delay to a finite value.
///
/// <center>
/// ```svgbob
///                   _____                               _____       _____
/// Timer callback   |_____|                             |_____|     |_____|
///                  1                                   2           3
///
///                  ├──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┼──┬──┬──┬──┤
///                  1  ↑                       ↑        2           3
///                delay ← None              delay ← 3
/// ​
/// ```
/// </center>
///
/// If [`period` is set] to `None` instead, the timer will stop firing after the
/// next tick.
///
/// <center>
/// ```svgbob
///                   _____       _____                   _____       _____
/// Timer callback   |_____|     |_____|                 |_____|     |_____|
///                  1           2                       3           4
///
///                  ├──┬──┬──┬──┤              ├──┬──┬──┼──┬──┬──┬──┤
///                  1  ↑                       ↑        3           4
///               period ← None  ├──┬──┬──┬──┬──┤
///                              2              ↑
///                                         period ← 4
///                                          delay ← 3
/// ​
/// ```
/// </center>
///
/// [`delay` is set]: Timer::set_delay
/// [`period` is set]: Timer::set_period
///
/// # Examples
///
/// ## Periodic Timer
///
/// ```rust
/// # #![feature(const_fn)]
/// # #![feature(const_mut_refs)]
/// # #![feature(const_fn_fn_ptr_basics)]
/// use r3::{kernel::{cfg::CfgBuilder, Timer, Kernel}, time::Duration};
///
/// const fn configure<System: Kernel>(b: &mut CfgBuilder<System>) -> Timer<System> {
///     Timer::build()
///         .delay(Duration::from_millis(70))
///         .period(Duration::from_millis(40))
///         .active(true)
///         .start(|_| dbg!())
///         .finish(b)
/// }
/// ```
///
/// <center>
/// ```svgbob
///                            _____       _____       _____       _____
/// Timer callback            |_____|     |_____|     |_____|     |_____|
///                           1           2           3           4
///
///      ├──┬──┬──┬──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┼──┬──┬──┬──┤
///      ↑        70ms        1   40ms    2   40ms    3   40ms    4   40ms
/// system boot
/// ​
/// ```
/// </center>
///
/// ## One-Shot Timer
///
/// ```rust
/// # #![feature(const_fn)]
/// # #![feature(const_mut_refs)]
/// # #![feature(const_fn_fn_ptr_basics)]
/// use r3::{kernel::{cfg::CfgBuilder, Timer, Kernel}, time::Duration};
///
/// const fn configure<System: Kernel>(b: &mut CfgBuilder<System>) -> Timer<System> {
///     Timer::build()
///         .active(true)
///         .start(|_| dbg!())
///         .finish(b)
/// }
/// ```
///
/// [Reset the delay] to schedule a call.
///
/// ```rust
/// use r3::{kernel::{Timer, Kernel}, time::Duration};
///
/// fn sched<System: Kernel>(timer: Timer<System>) {
///     timer.set_delay(Some(Duration::from_millis(40))).unwrap();
/// }
/// ```
///
/// <center>
/// ```svgbob
///                         _____                            _____
/// Timer callback         |_____|                          |_____|
///                        1                                2
///
///      ├──┬──┬──┬──┬──┬──┼──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┼──┬──┬──┬──┤
///            ↑   40ms    1           ↑        ↑   40ms    2
///          sched                   sched    sched
/// ​
/// ```
/// </center>
///
/// [Reset the delay]: Timer::set_delay
///
#[doc(include = "../common.md")]
#[repr(transparent)]
pub struct Timer<System>(Id, PhantomData<System>);

impl<System> Clone for Timer<System> {
    fn clone(&self) -> Self {
        Self(self.0, self.1)
    }
}

impl<System> Copy for Timer<System> {}

impl<System> PartialEq for Timer<System> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<System> Eq for Timer<System> {}

impl<System> hash::Hash for Timer<System> {
    fn hash<H>(&self, state: &mut H)
    where
        H: hash::Hasher,
    {
        hash::Hash::hash(&self.0, state);
    }
}

impl<System> fmt::Debug for Timer<System> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Timer").field(&self.0).finish()
    }
}

impl<System> Timer<System> {
    /// Construct a `Timer` from `Id`.
    ///
    /// # Safety
    ///
    /// The kernel can handle invalid IDs without a problem. However, the
    /// constructed `Timer` may point to an object that is not intended to be
    /// manipulated except by its creator. This is usually prevented by making
    /// `Timer` an opaque handle, but this safeguard can be circumvented by
    /// this method.
    pub const unsafe fn from_id(id: Id) -> Self {
        Self(id, PhantomData)
    }

    /// Get the raw `Id` value representing this timer.
    pub const fn id(self) -> Id {
        self.0
    }
}

impl<System: Kernel> Timer<System> {
    fn timer_cb(self) -> Result<&'static TimerCb<System>, BadIdError> {
        System::get_timer_cb(self.0.get() - 1).ok_or(BadIdError::BadId)
    }

    /// Start the timer (transition it into the Active state).
    ///
    /// This method has no effect if the timer is already in the Active state.
    #[cfg_attr(not(feature = "inline_syscall"), inline(never))]
    pub fn start(self) -> Result<(), StartTimerError> {
        let mut lock = lock_cpu::<System>()?;
        let timer_cb = self.timer_cb()?;
        start_timer(lock.borrow_mut(), timer_cb);
        Ok(())
    }

    /// Stop the timer (transition it into the Dormant state).
    ///
    /// This method has no effect if the timer is already in the Dormant state.
    #[cfg_attr(not(feature = "inline_syscall"), inline(never))]
    pub fn stop(self) -> Result<(), StopTimerError> {
        let mut lock = lock_cpu::<System>()?;
        let timer_cb = self.timer_cb()?;
        stop_timer(lock.borrow_mut(), timer_cb);
        Ok(())
    }

    /// Set the duration before the next tick.
    ///
    /// If the timer is currently in the Dormant state, this method specifies
    /// the duration between the next activation and the first tick
    /// following the activation.
    ///
    /// `None` means infinity (the timer will never fire).
    #[cfg_attr(not(feature = "inline_syscall"), inline(never))]
    pub fn set_delay(self, delay: Option<Duration>) -> Result<(), SetTimerDelayError> {
        let time32 = if let Some(x) = delay {
            timeout::time32_from_duration(x)?
        } else {
            timeout::BAD_DURATION32
        };
        let mut lock = lock_cpu::<System>()?;
        let timer_cb = self.timer_cb()?;
        set_timer_delay(lock.borrow_mut(), timer_cb, time32);
        Ok(())
    }

    /// Set the timer period, which is a quantity to be added to the timer's
    /// absolute arrival time on every tick.
    ///
    /// `None` means infinity.
    #[cfg_attr(not(feature = "inline_syscall"), inline(never))]
    pub fn set_period(self, period: Option<Duration>) -> Result<(), SetTimerPeriodError> {
        let time32 = if let Some(x) = period {
            timeout::time32_from_duration(x)?
        } else {
            timeout::BAD_DURATION32
        };
        let mut lock = lock_cpu::<System>()?;
        let timer_cb = self.timer_cb()?;
        set_timer_period(lock.borrow_mut(), timer_cb, time32);
        Ok(())
    }
}

/// *Timer control block* - the state data of a timer.
///
/// This type isn't technically public but needs to be `pub` so that it can be
/// referred to by a macro.
#[doc(hidden)]
pub struct TimerCb<System: Kernel> {
    /// The static properties of the timer.
    pub(super) attr: &'static TimerAttr<System>,

    /// The timeout object for the timer.
    ///
    ///  - If the delay is `Some(_)` and the timer is in the Active state, the
    ///    timeout object is linked. The delay is implicitly defined in this
    ///    case.
    ///
    ///  - If the delay is `None` or the timer is in the Dormant state, the
    ///    timeout object is unlinked. The delay can be retrieved by
    ///    [`timeout::Timeout::at_raw`].
    ///
    // FIXME: `!Drop` is a requirement of `array_item_from_fn!` that ideally
    //        should be removed
    pub(super) timeout: ManuallyDrop<timeout::Timeout<System>>,

    /// `true` iff the timer is in the Active state.
    pub(super) active: CpuLockCell<System, bool>,

    pub(super) period: CpuLockCell<System, timeout::Time32>,
}

impl<System: Kernel> Init for TimerCb<System> {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = Self {
        attr: &Init::INIT,
        timeout: Init::INIT,
        active: Init::INIT,
        period: Init::INIT,
    };
}

impl<System: Kernel> fmt::Debug for TimerCb<System> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TimerCb")
            .field("self", &(self as *const _))
            .field("attr", &self.attr)
            .field("timeout", &self.timeout)
            .field("active", &self.active)
            .field("period", &self.period)
            .finish()
    }
}

/// The static properties of a timer.
///
/// This type isn't technically public but needs to be `pub` so that it can be
/// referred to by a macro.
#[doc(hidden)]
pub struct TimerAttr<System> {
    /// The entry point of the timer.
    ///
    /// # Safety
    ///
    /// This is only meant to be used by a kernel port, as a timer callback,
    /// not by user code. Using this in other ways may cause an undefined
    /// behavior.
    pub(super) entry_point: fn(usize),

    /// The parameter supplied for `entry_point`.
    pub(super) entry_param: usize,

    /// The initial state of the timer.
    pub(super) init_active: bool,

    pub(super) _phantom: PhantomData<System>,
}

impl<System> Init for TimerAttr<System> {
    const INIT: Self = Self {
        entry_point: |_| {},
        entry_param: 0,
        init_active: false,
        _phantom: PhantomData,
    };
}

impl<System: Kernel> fmt::Debug for TimerAttr<System> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TimerAttr")
            .field("entry_point", &self.entry_point)
            .field("entry_param", &self.entry_param)
            .finish()
    }
}

/// Initialize a timer at boot time.
pub(super) fn init_timer<System: Kernel>(
    mut lock: CpuLockTokenRefMut<'_, System>,
    timer_cb: &'static TimerCb<System>,
) {
    if timer_cb.attr.init_active {
        // Get the initial delay value
        let delay = timer_cb.timeout.at_raw(lock.borrow_mut());

        if delay != timeout::BAD_DURATION32 {
            // Schedule the first tick
            timeout::insert_timeout(lock.borrow_mut(), static_pin(&timer_cb.timeout));
        }

        timer_cb.active.replace(&mut *lock, true);
    }
}

/// The core portion of [`Timer::start`].
fn start_timer<System: Kernel>(
    mut lock: CpuLockTokenRefMut<'_, System>,
    timer_cb: &'static TimerCb<System>,
) {
    if timer_cb.active.get(&*lock) {
        return;
    }

    // Get the current delay value
    let delay = timer_cb.timeout.at_raw(lock.borrow_mut());

    if delay != timeout::BAD_DURATION32 {
        // Schedule the next tick
        timer_cb
            .timeout
            .set_expiration_after(lock.borrow_mut(), delay);
        timeout::insert_timeout(lock.borrow_mut(), static_pin(&timer_cb.timeout));
    }

    timer_cb.active.replace(&mut *lock, true);
}

/// The core portion of [`Timer::stop`].
fn stop_timer<System: Kernel>(
    mut lock: CpuLockTokenRefMut<'_, System>,
    timer_cb: &TimerCb<System>,
) {
    if timer_cb.timeout.is_linked(lock.borrow_mut()) {
        debug_assert!(timer_cb.active.get(&*lock));

        // Capture the current delay value
        let delay = timer_cb
            .timeout
            .saturating_duration_until_timeout(lock.borrow_mut());

        // Unlink the timeout
        timeout::remove_timeout(lock.borrow_mut(), &timer_cb.timeout);

        // Store the captured delay value
        timer_cb.timeout.set_at_raw(lock.borrow_mut(), delay);
    }

    timer_cb.active.replace(&mut *lock, false);
}

/// The core portion of [`Timer::set_delay`].
fn set_timer_delay<System: Kernel>(
    mut lock: CpuLockTokenRefMut<'_, System>,
    timer_cb: &'static TimerCb<System>,
    delay: timeout::Time32,
) {
    let is_active = timer_cb.active.get(&*lock);

    if timer_cb.timeout.is_linked(lock.borrow_mut()) {
        timeout::remove_timeout(lock.borrow_mut(), &timer_cb.timeout);
    }

    if is_active && delay != timeout::BAD_DURATION32 {
        timer_cb
            .timeout
            .set_expiration_after(lock.borrow_mut(), delay);
        timeout::insert_timeout(lock.borrow_mut(), static_pin(&timer_cb.timeout));
    } else {
        timer_cb.timeout.set_at_raw(lock.borrow_mut(), delay);
    }
}

/// The core portion of [`Timer::set_period`].
fn set_timer_period<System: Kernel>(
    mut lock: CpuLockTokenRefMut<'_, System>,
    timer: &TimerCb<System>,
    period: timeout::Time32,
) {
    timer.period.replace(&mut *lock, period);
}

/// The timeout callback function for a timer. This function should be
/// registered as a callback function when initializing [`TimerCb::timeout`].
///
/// `i` is an index into [`super::KernelCfg2::timer_cb_pool`].
pub(super) fn timer_timeout_handler<System: Kernel>(
    i: usize,
    mut lock: CpuLockGuard<System>,
) -> CpuLockGuard<System> {
    let timer_cb = System::get_timer_cb(i).unwrap();

    // Schedule the next tick
    debug_assert!(!timer_cb.timeout.is_linked(lock.borrow_mut()));
    debug_assert!(timer_cb.active.get(&*lock));

    let period = timer_cb.period.get(&*lock);
    if period == timeout::BAD_DURATION32 {
        timer_cb
            .timeout
            .set_at_raw(lock.borrow_mut(), timeout::BAD_DURATION32);
    } else {
        timer_cb
            .timeout
            .adjust_expiration(lock.borrow_mut(), period);
        timeout::insert_timeout(lock.borrow_mut(), static_pin(&timer_cb.timeout));
    }

    // Release CPU Lock before calling the application-provided callback
    // function
    drop(lock);

    let TimerAttr {
        entry_point,
        entry_param,
        ..
    } = timer_cb.attr;
    entry_point(*entry_param);

    // Re-acquire CPU Lock
    lock_cpu().unwrap_or_else(|_| unsafe { assume_cpu_lock() })
}
