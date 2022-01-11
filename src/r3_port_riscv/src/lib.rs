#![feature(const_fn_trait_bound)]
#![feature(decl_macro)]
#![feature(naked_functions)]
#![feature(slice_ptr_len)]
#![feature(asm_const)]
#![feature(asm_sym)]
#![feature(raw_ref_op)]
#![feature(const_option)]
#![feature(const_mut_refs)]
#![feature(const_fn_fn_ptr_basics)]
#![feature(const_trait_impl)]
#![feature(generic_const_exprs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(unsupported_naked_functions)]
#![doc(html_logo_url = "https://r3-os.github.io/r3/logo-small.svg")]
#![doc = include_str!("./lib.md")]
#![doc = include_str!("./common.md")]
#![doc = include!("../doc/interrupts.rs")] // `![interrupts]`
#![recursion_limit = "1024"]
#![no_std]
use core::ops::Range;
use r3_core::kernel::{
    ClearInterruptLineError, EnableInterruptLineError, InterruptNum, InterruptPriority,
    PendInterruptLineError, QueryInterruptLineError, SetInterruptLinePriorityError,
};

/// Used by macros
#[doc(hidden)]
pub extern crate r3_core;

/// Used by macros
#[doc(hidden)]
pub extern crate r3_kernel;

/// Used by macros
#[doc(hidden)]
pub extern crate r3_portkit;

/// Used by macros
#[doc(hidden)]
pub extern crate core;

#[cfg(doc)]
#[doc = include_str!("../CHANGELOG.md")]
pub mod _changelog_ {}

/// The Platform-Level Interrupt Controller driver.
#[doc(hidden)]
pub mod plic {
    pub mod cfg;
    pub mod imp;
    pub mod plic_regs;
}

/// The binding for [`::riscv_rt`].
#[doc(hidden)]
pub mod rt {
    pub mod cfg;
    #[cfg(target_os = "none")]
    pub mod imp;
}

/// The [`r3_kernel::PortThreading`] implementation.
#[doc(hidden)]
pub mod threading {
    pub mod cfg;
    #[cfg(target_os = "none")]
    pub mod imp;
}

/// The `mtime`-based timer driver.
#[doc(hidden)]
pub mod mtime {
    pub mod cfg;
    pub mod imp;
}

/// The SBI-based timer driver.
#[doc(hidden)]
pub mod sbi_timer {
    pub mod cfg;
    pub mod imp;
}

pub use self::mtime::cfg::*;
pub use self::plic::cfg::*;
pub use self::rt::cfg::*;
pub use self::sbi_timer::cfg::*;
pub use self::threading::cfg::*;

/// Defines the entry points of a port instantiation. Implemented by
/// [`use_port!`].
pub trait EntryPoint {
    /// Proceed with the boot process.
    ///
    /// # Safety
    ///
    ///  - The processor should be in the privilege mode specified by
    ///    [`ThreadingOptions::PRIVILEGE_LEVEL`] and have interrupts masked for
    ///    this privilege level.
    ///  - This method hasn't been entered yet.
    ///
    unsafe fn start() -> !;

    /// The trap handler.
    ///
    /// It's aligned to a 4-byte boundary so that it can be set to `mtvec`.
    ///
    /// # Safety
    ///
    ///  - The processor should be in the privilege mode specified by
    ///    [`ThreadingOptions::PRIVILEGE_LEVEL`] and have interrupts masked for
    ///    this privilege level
    ///  - The register state of the background context should be preserved so
    ///    that the handler can restore it later.
    ///
    const TRAP_HANDLER: unsafe extern "C" fn() -> !;
}

/// An abstract inferface to a port timer driver. Implemented by
/// [`use_mtime!`] and [`use_sbi_timer!`].
pub trait Timer {
    /// Initialize the driver. This will be called just before entering
    /// [`PortToKernel::boot`].
    ///
    /// [`PortToKernel::boot`]: r3_kernel::PortToKernel::boot
    ///
    /// # Safety
    ///
    /// This is only intended to be called by the port.
    unsafe fn init() {}
}

/// An abstract interface to an interrupt controller. Implemented by
/// [`use_plic!`].
///
/// # Safety
///
/// These methods are only intended to be called by the port.
pub trait InterruptController {
    /// Initialize the driver. This will be called just before entering
    /// [`PortToKernel::boot`].
    ///
    /// [`PortToKernel::boot`]: r3_kernel::PortToKernel::boot
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn init() {}

    /// The range of interrupt priority values considered [managed].
    ///
    /// Defaults to `0..0` (empty) when unspecified.
    ///
    /// [managed]: r3_core#interrupt-handling-framework
    #[allow(clippy::reversed_empty_ranges)] // on purpose
    const MANAGED_INTERRUPT_PRIORITY_RANGE: Range<InterruptPriority> = 0..0;

    /// Handle the call to [`PortInterrupts::set_interrupt_line_priority`] for a
    /// platform interrupt line.
    ///
    /// The provided interrupt number must be greater than or equal to
    /// [`INTERRUPT_PLATFORM_START`].
    ///
    /// [`PortInterrupts::set_interrupt_line_priority`]: r3_kernel::PortInterrupts::set_interrupt_line_priority
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn set_interrupt_line_priority(
        _line: InterruptNum,
        _priority: InterruptPriority,
    ) -> Result<(), SetInterruptLinePriorityError> {
        Err(SetInterruptLinePriorityError::BadParam)
    }

    /// Handle the call to [`PortInterrupts::enable_interrupt_line`] for a
    /// platform interrupt line.
    ///
    /// The provided interrupt number must be greater than or equal to
    /// [`INTERRUPT_PLATFORM_START`].
    ///
    /// [`PortInterrupts::enable_interrupt_line`]: r3_kernel::PortInterrupts::enable_interrupt_line
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn enable_interrupt_line(_line: InterruptNum) -> Result<(), EnableInterruptLineError> {
        Err(EnableInterruptLineError::BadParam)
    }

    /// Handle the call to [`PortInterrupts::disable_interrupt_line`] for a
    /// platform interrupt line.
    ///
    /// The provided interrupt number must be greater than or equal to
    /// [`INTERRUPT_PLATFORM_START`].
    ///
    /// [`PortInterrupts::disable_interrupt_line`]: r3_kernel::PortInterrupts::disable_interrupt_line
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn disable_interrupt_line(_line: InterruptNum) -> Result<(), EnableInterruptLineError> {
        Err(EnableInterruptLineError::BadParam)
    }

    /// Handle the call to [`PortInterrupts::pend_interrupt_line`] for a
    /// platform interrupt line.
    ///
    /// The provided interrupt number must be greater than or equal to
    /// [`INTERRUPT_PLATFORM_START`].
    ///
    /// [`PortInterrupts::pend_interrupt_line`]: r3_kernel::PortInterrupts::pend_interrupt_line
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn pend_interrupt_line(_line: InterruptNum) -> Result<(), PendInterruptLineError> {
        Err(PendInterruptLineError::BadParam)
    }

    /// Handle the call to [`PortInterrupts::clear_interrupt_line`] for a
    /// platform interrupt line.
    ///
    /// The provided interrupt number must be greater than or equal to
    /// [`INTERRUPT_PLATFORM_START`].
    ///
    /// [`PortInterrupts::clear_interrupt_line`]: r3_kernel::PortInterrupts::clear_interrupt_line
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn clear_interrupt_line(_line: InterruptNum) -> Result<(), ClearInterruptLineError> {
        Err(ClearInterruptLineError::BadParam)
    }

    /// Handle the call to [`PortInterrupts::is_interrupt_line_pending`] for a
    /// platform interrupt line.
    ///
    /// The provided interrupt number must be greater than or equal to
    /// [`INTERRUPT_PLATFORM_START`].
    ///
    /// [`PortInterrupts::is_interrupt_line_pending`]: r3_kernel::PortInterrupts::is_interrupt_line_pending
    ///
    /// # Safety
    ///
    /// See this trait's documentation.
    unsafe fn is_interrupt_line_pending(
        _line: InterruptNum,
    ) -> Result<bool, QueryInterruptLineError> {
        Err(QueryInterruptLineError::BadParam)
    }
}
