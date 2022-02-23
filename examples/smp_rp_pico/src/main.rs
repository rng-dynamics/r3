#![feature(const_fn_fn_ptr_basics)]
#![feature(const_fn_trait_bound)]
#![feature(const_refs_to_cell)]
#![feature(const_trait_impl)]
#![feature(const_mut_refs)]
#![feature(asm_sym)]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_std]
#![no_main]
#![cfg(target_os = "none")]

mod core0;
mod core1;
mod panic_serial;

// The second-level bootloader, which is responsible for configuring execute-in-
// place. The bootrom copies this into SRAM and executes it.
#[link_section = ".boot_loader"]
#[used]
pub static BOOT_LOADER: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;
