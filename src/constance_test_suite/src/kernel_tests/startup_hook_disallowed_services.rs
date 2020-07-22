//! Checks the return codes of disallowed system calls made in a boot context.
use constance::{
    kernel::{self, cfg::CfgBuilder, StartupHook},
    prelude::*,
};
use core::marker::PhantomData;

use super::Driver;

pub struct App<System> {
    _phantom: PhantomData<System>,
}

impl<System: Kernel> App<System> {
    pub const fn new<D: Driver<Self>>(b: &mut CfgBuilder<System>) -> Self {
        StartupHook::build().start(hook::<System, D>).finish(b);

        App {
            _phantom: PhantomData,
        }
    }
}

fn hook<System: Kernel, D: Driver<App<System>>>(_: usize) {
    // Disallowed in a non-task context
    assert_eq!(
        System::boost_priority(),
        Err(kernel::BoostPriorityError::BadContext),
    );
    assert_eq!(
        unsafe { System::exit_task() },
        Err(kernel::ExitTaskError::BadContext),
    );

    // Blocking system services
    assert_eq!(System::park(), Err(kernel::ParkError::BadContext));

    D::success();
}
