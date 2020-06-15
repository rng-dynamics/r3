//! Validates error codes returned by task manipulation methods. Also, checks
//! miscellaneous properties of `Task`.
use constance::{kernel::Task, prelude::*};
use core::num::NonZeroUsize;
use wyhash::WyHash;

use super::Driver;

pub struct App<System> {
    task1: Task<System>,
    task2: Task<System>,
}

impl<System: Kernel> App<System> {
    constance::configure! {
        pub const fn new<D: Driver<Self>>(_: &mut CfgBuilder<System>) -> Self {
            let task1 = build! {
                Task<_>,
                start = task1_body::<System, D>,
                priority = 2,
                active = true,
                param = 42,
            };
            let task2 = build! { Task<_>, start = task2_body::<System, D>, priority = 1 };

            App { task1, task2 }
        }
    }
}

fn task1_body<System: Kernel, D: Driver<App<System>>>(param: usize) {
    assert_eq!(param, 42);

    // `PartialEq`
    let app = D::app();
    assert_ne!(app.task1, app.task2);
    assert_eq!(app.task1, app.task1);
    assert_eq!(app.task2, app.task2);

    // `Hash`
    let hash = |x: Task<System>| {
        use core::hash::{Hash, Hasher};
        let mut hasher = WyHash::with_seed(42);
        x.hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(hash(app.task1), hash(app.task1));
    assert_eq!(hash(app.task2), hash(app.task2));

    // Invalid task ID
    let bad_task: Task<System> = unsafe { Task::from_id(NonZeroUsize::new(42).unwrap()) };
    assert_eq!(
        bad_task.activate(),
        Err(constance::kernel::ActivateTaskError::BadId)
    );

    // The task is already active
    assert_eq!(
        app.task1.activate(),
        Err(constance::kernel::ActivateTaskError::QueueOverflow)
    );

    // The task is dormant
    assert_eq!(
        app.task2.interrupt(),
        Err(constance::kernel::InterruptTaskError::BadObjectState)
    );
    assert_eq!(
        app.task2.cancel_interrupt(),
        Err(constance::kernel::CancelInterruptTaskError::BadObjectState)
    );

    // Enqueue an interrupt request
    app.task1.interrupt().unwrap();

    // An interrupt request is already enqueued
    assert_eq!(
        app.task1.interrupt(),
        Err(constance::kernel::InterruptTaskError::QueueOverflow)
    );

    // Cancel an interrupt request
    assert_eq!(app.task1.cancel_interrupt(), Ok(1));

    // Cancel a non-existent interrupt request
    assert_eq!(app.task1.cancel_interrupt(), Ok(0));

    // Current task
    assert_eq!(Task::current().unwrap(), Some(app.task1));

    D::success();
}

fn task2_body<System: Kernel, D: Driver<App<System>>>(_: usize) {
    unreachable!();
}
