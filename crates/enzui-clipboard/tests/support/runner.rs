//! Serial runner with named results, panic capture and nonzero failure exits.
//!
//! The last positional argument supplies the substring filter; `--exact`
//! selects whole names. Other flag strings are ignored. A zero-match filter
//! exits with failure. Output is never captured.

use std::io::Write;
use std::time::Instant;

pub struct Case {
    pub name: &'static str,
    pub run: fn(),
}

pub fn run(cases: &[Case]) -> ! {
    let mut filter: Option<String> = None;
    let mut exact = false;

    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--exact" => exact = true,
            flag if flag.starts_with('-') => {}
            name => filter = Some(name.to_owned()),
        }
    }

    let selected: Vec<&Case> = cases
        .iter()
        .filter(|case| match &filter {
            None => true,
            Some(filter) if exact => case.name == filter,
            Some(filter) => case.name.contains(filter),
        })
        .collect();

    if selected.is_empty() {
        eprintln!(
            "error: the filter {:?} matched none of the {} cases in this target",
            filter.unwrap_or_default(),
            cases.len()
        );
        std::process::exit(101);
    }

    let filtered_out = cases.len() - selected.len();
    let started = Instant::now();
    let mut failed = Vec::new();

    println!("\nrunning {} tests", selected.len());

    for case in &selected {
        print!("test {} ... ", case.name);
        let _ = std::io::stdout().flush();

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(case.run)) {
            Ok(()) => println!("ok"),
            Err(_) => {
                println!("FAILED");
                failed.push(case.name);
            }
        }
    }

    let elapsed = started.elapsed();

    if !failed.is_empty() {
        println!("\nfailures:");

        for name in &failed {
            println!("    {name}");
        }
    }

    println!(
        "\ntest result: {}. {} passed; {} failed; 0 ignored; 0 measured; {filtered_out} filtered out; \
         finished in {:.2}s\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len(),
        elapsed.as_secs_f64()
    );

    std::process::exit(if failed.is_empty() { 0 } else { 101 });
}
