//! Compiles the trimmed mecab-ipadic that `tools/make_ja_dict.py` writes into
//! a vibrato dictionary and packs it with zstd (level 19). Run by that script:
//!
//!     ja-dict LEX.csv MATRIX.def CHAR.def UNK.def OUT.dic.zst

use std::fs::File;
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        eprintln!("usage: ja-dict LEX.csv MATRIX.def CHAR.def UNK.def OUT.dic.zst");
        exit(2);
    }
    let open = |path: &str| {
        File::open(path).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            exit(1)
        })
    };
    let dict = vibrato::SystemDictionaryBuilder::from_readers(open(&args[0]), open(&args[1]), open(&args[2]), open(&args[3]))
        .unwrap_or_else(|e| {
            eprintln!("compiling the dictionary failed: {e}");
            exit(1)
        });
    let mut raw = Vec::new();
    dict.write(&mut raw).unwrap_or_else(|e| {
        eprintln!("writing the dictionary failed: {e}");
        exit(1)
    });
    let packed = zstd::bulk::compress(&raw, 19).unwrap_or_else(|e| {
        eprintln!("packing failed: {e}");
        exit(1)
    });
    if let Err(e) = std::fs::write(&args[4], &packed) {
        eprintln!("{}: {e}", args[4]);
        exit(1);
    }
    println!("{} bytes unpacked, {} packed", raw.len(), packed.len());
}
