#![feature(external_doc)] // `#[doc(include = ...)]`
#![feature(const_loop)]
#![feature(const_fn)]
#![feature(const_if_match)]
#![feature(unsafe_block_in_unsafe_fn)] // `unsafe fn` doesn't imply `unsafe {}`
#![deny(unsafe_op_in_unsafe_fn)]
#![doc(include = "./lib.md")]
#![no_std]

mod utils;

// Generated by `build.rs`. Defines `get_selected_kernel_tests_inner!`.
include!(concat!(env!("OUT_DIR"), "/selective_tests.rs"));

/// Kernel tests
pub mod kernel_tests {
    /// Instantiation parameters of a test case.
    ///
    /// This trait has two purposes: (1) It serves as an interface to a test driver.
    /// It provides methods to notify the test driver of test success or failure.
    /// (2) It provides runtime access to the `App` structure.
    pub trait Driver<App> {
        /// Get a reference to `App` of the current test case.
        fn app() -> &'static App;

        /// Signal to the test runner that a test has succeeded.
        fn success();

        /// Signal to the test runner that a test has failed.
        fn fail();
    }

    macro_rules! define_kernel_tests {
        (
            [$dollar:tt] // get a `$` token
            $(
                // Test case definition
                (mod $name_ident:ident {}, $name_str:literal)
            ),*$(,)*
        ) => {
            $(
                /// [**Test Case**]
                #[cfg(any(
                    feature = "tests_all",
                    all(feature = "tests_selective", kernel_test = $name_str)
                ))]
                pub mod $name_ident;
            )*

            /// The names of kernel tests.
            pub const TEST_NAMES: &[&str] = &[
                $( $name_str ),*
            ];

            /// Invoke the specified macro with a description of all defined
            /// kernel test cases.
            ///
            /// Note that the tests might not be actually compiled unless the
            /// feature `tests_all` is enabled.
            ///
            /// # Example
            ///
            /// ```rust,ignore
            /// constance_test_suite::get_kernel_tests!(aaa::bbb!(prefix));
            /// ```
            ///
            /// This expands to something like this:
            ///
            /// ```rust,ignore
            /// aaa::bbb!(
            ///     prefix
            ///     { name_ident: test1, name_str: "test1", },
            ///     { name_ident: test2, name_str: "test2", },
            /// );
            /// ```
            ///
            #[macro_export]
            macro_rules! get_kernel_tests {
                (
                    // Callback macro
                    $path:ident$dollar(::$path_sub:ident)* ! (
                        // Prefix of the callback parameter
                        $dollar($prefix:tt)*
                    )
                ) => {
                    $path$dollar($path_sub)* ! (
                        // Prefix
                        $dollar($prefix)*
                        $(
                            // The test info
                            {
                                name_ident: $name_ident,
                                name_str: $name_str,
                            },
                        )*
                    );
                };
            }
        };
    }

    define_kernel_tests! {
        [$]
        (mod basic {}, "basic"),
        (mod event_group_misc {}, "event_group_misc"),
        (mod event_group_order_fifo {}, "event_group_order_fifo"),
        (mod event_group_order_task_priority {}, "event_group_order_task_priority"),
        (mod event_group_set_and_dispatch {}, "event_group_set_and_dispatch"),
        (mod event_group_wait_types {}, "event_group_wait_types"),
        (mod task_activate_and_dispatch {}, "task_activate_and_dispatch"),
        (mod task_activate_and_do_not_dispatch {}, "task_activate_and_do_not_dispatch"),
        (mod task_misc {}, "task_misc"),
        (mod task_queue_fifo {}, "task_queue_fifo"),
    }

    /// Invoke the specified macro with a description of test cases
    /// selected by `CONSTANCE_TEST`.
    ///
    /// Note that the tests might not be actually compiled unless the
    /// feature `tests_selective` is enabled.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// constance_test_suite::get_selected_kernel_tests!(aaa::bbb!(prefix));
    /// ```
    ///
    /// If there's an environment variable `CONSTANCE_TEST=kernel_test::test1`,
    /// this expands to:
    ///
    /// ```rust,ignore
    /// aaa::bbb!(
    ///     prefix
    ///     { name_ident: test1, name_str: "test1", },
    /// );
    /// ```
    ///
    #[macro_export]
    macro_rules! get_selected_kernel_tests {
        (
            // Callback macro
            $path:ident$(::$path_sub:ident)* ! (
                // Prefix of the callback parameter
                $($prefix:tt)*
            )
        ) => {
            // Forward to `get_selected_kernel_tests_inner!`
            $crate::get_selected_kernel_tests_inner!(
                ( $path $(::$path_sub)* ), ( $($prefix)* )
            );
        };
    }
}
