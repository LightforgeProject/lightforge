#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| lightforge_core::fuzzing::idx_file_name(bytes));
