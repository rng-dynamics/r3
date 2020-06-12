use std::{env, fmt, fs, path::Path};

fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();

    println!("cargo:rerun-if-changed=build.rs");

    // Selective building
    println!("cargo:rerun-if-env-changed=CONSTANCE_TEST");

    let selected_tests_joined = match env::var("CONSTANCE_TEST") {
        Ok(x) => x,
        Err(env::VarError::NotPresent) => String::new(),
        Err(env::VarError::NotUnicode(_)) => {
            panic!("CONSTANCE_TEST is not a valid UTF-8 string");
        }
    };
    let selected_tests = selected_tests_joined
        .trim()
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let mut kernel_test_list = Vec::new();

    for selected_test in selected_tests {
        if let Some(name) = selected_test.strip_prefix("kernel_tests::") {
            expect_valid_test_name(name);

            // Enable `cfg(kernel_test = "...")`
            println!("cargo:rustc-cfg=kernel_test=\"{}\"", name);

            // Include it in `get_selected_kernel_tests_inner`
            kernel_test_list.push(TestMeta(name));
        } else {
            panic!(
                "Unrecognized test type: `{}`
                Test names should start with a prefix like `kernel_tests::`.",
                selected_test
            );
        }
    }

    let out_selective_tests_path = Path::new(&out_dir).join("selective_tests.rs");
    fs::write(
        &out_selective_tests_path,
        &format!(
            "#[macro_export]
            #[doc(hidden)]
            macro_rules! get_selected_kernel_tests_inner {{
                (($($cb:tt)*), ($($pfx:tt)*)) => {{
                    $($cb:tt)* ! ( $($pfx:tt)*
                        {}
                    )
                }};
            }}",
            CommaSeparatedWithTrailingComma(&kernel_test_list)
        ),
    )
    .unwrap();
}

fn expect_valid_test_name(name: &str) {
    if name.contains(|c: char| !c.is_alphanumeric() && c != '_') || name.is_empty() {
        panic!(
            "Invalid test name: `{}`
            Test names should match /[a-zA-Z0-9_]+/",
            name
        )
    }
}

struct TestMeta<'a>(&'a str);

impl fmt::Display for TestMeta<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{ name_ident: {0}, name_str: \"{0}\", }}", self.0)
    }
}

struct CommaSeparatedWithTrailingComma<T>(T);
impl<T> fmt::Display for CommaSeparatedWithTrailingComma<T>
where
    T: Clone + IntoIterator,
    T::Item: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for e in self.0.clone() {
            write!(f, "{}, ", e)?;
        }
        Ok(())
    }
}
