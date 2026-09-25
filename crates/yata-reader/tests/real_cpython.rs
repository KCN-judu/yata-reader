//! The desktop channel against a real process: a CPython interpreter of the layout the reader
//! reads (3.6 to 3.11) holds a made-up inventory, and the reader finds it from outside, through
//! the same attach, discovery, and soul reading the game gets. Only the game itself is missing.
//!
//! It runs where `YATA_TEST_PYTHON` names such an interpreter; the Windows CI job installs one.
//! Elsewhere it says that it did not run.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use yata_protocol::probe::reading::Records;
use yata_protocol::probe::{RawValue, raw_value::Kind};
use yata_reader::desktop;
use yata_reader::layout::cpython::discover;
use yata_reader::layout::memory::Cached;
use yata_reader::layout::souls::read_souls;
use yata_reader::session::Target;

const SCRIPT: &str = r#"
import sys, time
def soul(n, innate):
    return {"base_rindex": n, "rattr": [(i + 1, 0.5 * i) for i in range(n % 5)],
            "single_attr": innate, "others": (1 << 40) | n, "sattr": f"synthetic-{n}",
            "base_r": 10.0 * n}
inventory = {f"{i:024x}": soul(i, 7 if i % 2 == 0 else None) for i in range(1, 51)}
stray = soul(99, 9)
stray["wide"] = "御魂 café \U0001d11e"
stray["big"] = 1 << 70
with open(sys.argv[1], "w") as f:
    f.write("ready")
time.sleep(120)
"#;

struct Scratch {
    dir: PathBuf,
    child: Option<Child>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(v: Option<&RawValue>) -> Option<&str> {
    match v?.kind.as_ref()? {
        Kind::Text(t) => Some(t),
        _ => None,
    }
}

#[test]
fn a_real_cpython_inventory_is_read_from_outside() {
    let Ok(python) = std::env::var("YATA_TEST_PYTHON") else {
        eprintln!("not run: YATA_TEST_PYTHON does not name a CPython 3.6-3.11 interpreter");
        return;
    };
    let dir = std::env::temp_dir().join(format!("yata-reader-real-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut s = Scratch { dir, child: None };
    let script = s.dir.join("inventory.py");
    let ready = s.dir.join("ready");
    std::fs::write(&script, SCRIPT).expect("script");
    s.child = Some(
        Command::new(&python)
            .arg(&script)
            .arg(&ready)
            .spawn()
            .expect("the interpreter starts"),
    );
    let start = Instant::now();
    while !ready.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the script never got ready"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid = s.child.as_ref().map(Child::id).expect("started");

    let target = Target::Pid(std::num::NonZeroU32::new(pid).expect("a live pid"));
    let (chosen, memory) = desktop::attach(target).expect("our own child opens with read rights");
    assert_eq!(chosen.pid, pid);
    let memory = Cached::new(memory);
    let runtime = discover(&memory).expect("a CPython 3.6-3.11 runtime");
    let result = read_souls(&memory, &runtime, &mut |_, _| (), &|| false).expect("read");

    let Some(Records::Souls(souls)) = &result.records else {
        panic!("souls")
    };
    assert_eq!(
        souls.souls.len(),
        51,
        "fifty in the inventory and one stray"
    );
    let keyed: Vec<&str> = souls
        .souls
        .iter()
        .filter_map(|r| text(r.observed.as_ref()?.container_key.as_ref()))
        .collect();
    assert_eq!(keyed.len(), 50);
    assert_eq!(keyed[0], "000000000000000000000001");
    assert_eq!(keyed[49], "000000000000000000000032");
    // The runtime's table of interned names holds the marker keys with texts as values.
    assert!(result.stats.expect("stats").candidates_rejected >= 1);
    let stray = souls.souls[50].observed.as_ref().expect("observed");
    let wide = stray
        .entries
        .iter()
        .find(|e| text(e.key.as_ref()) == Some("wide"))
        .and_then(|e| text(e.value.as_ref()));
    assert_eq!(wide, Some("御魂 café 𝄞"));
}
