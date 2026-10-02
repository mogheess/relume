//! Probes every file in testdata/work/corpus (generate with testdata/make_corpus.sh).
//! Each must be identified, fully validated, and measured to its exact size.

use std::path::Path;

use relume_core::device::Reader;
use relume_core::formats;

#[test]
fn corpus_files_validate_exactly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/work/corpus");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("corpus missing; run testdata/make_corpus.sh");
        return;
    };
    let mut failures = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let dev = relume_core::open_path(&p).unwrap();
        let size = dev.len();
        let mut r = Reader::new(dev);
        match formats::probe(&mut r, 0, size) {
            Some(pr) if pr.complete && pr.len == size => {
                println!("ok   {:<24} {:?} {}", p.file_name().unwrap().to_string_lossy(), pr.format, pr.info.summary())
            }
            Some(pr) => failures.push(format!(
                "{}: {:?} len {} of {} complete={} note={:?}",
                p.display(), pr.format, pr.len, size, pr.complete, pr.note
            )),
            None => failures.push(format!("{}: not recognised", p.display())),
        }
    }
    assert!(failures.is_empty(), "{:#?}", failures);
}
