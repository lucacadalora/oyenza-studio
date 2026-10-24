#!/usr/bin/env python3
"""Build crates/poni-core/data/ja-readings.dic.zst, Oyen Poni's own Japanese
reading dictionary, from mecab-ipadic 2.7.0-20070801.

Usage:
    python3 tools/make_ja_dict.py              # download the pinned source, rebuild the file
    python3 tools/make_ja_dict.py --src FILE   # use a local copy of the source tarball
    python3 tools/make_ja_dict.py --check      # rebuild to a temp file, compare with the committed one

Steps:
1. Fetch mecab-ipadic (Debian's copy of the original tarball) and check its SHA-256.
2. Keep what lyrics need: drop person and company names (Noun.name, Noun.org);
   keep place names and other proper nouns only when common (cost below 8000,
   so 東京 and 渋谷 stay).
3. Cut each entry's features to "<class>" or "<class>,<reading>", the reading
   (katakana) only when the surface has kanji. romanize/ja_dict.rs reads the
   classes:
     N  content word (starts a phrase)    P  particle
     A  auxiliary verb                    T  te/ta ending (joins its verb)
     J  suffix or dependent word          F  prefix (the next word joins it)
     S  symbol
4. Compile it with vibrato and pack it with zstd (tools/ja-dict, built with cargo).

Needs Python 3 (standard library only) and cargo. The same input always gives
the same bytes, which is what --check relies on. mecab-ipadic's license
(NAIST, with ICOT's terms) allows changing and redistributing it as long as
every copy carries its notice: see THIRD-PARTY-NOTICES.txt.
"""

import csv
import hashlib
import io
import os
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

SOURCE = "https://deb.debian.org/debian/pool/main/m/mecab-ipadic/mecab-ipadic_2.7.0-20070801+main.orig.tar.gz"
SHA256 = "b62f527d881c504576baed9c6ef6561554658b175ce6ae0096a60307e49e3523"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "crates", "poni-core", "data", "ja-readings.dic.zst")

# Person and company names: rare in lyrics, a quarter of the entries.
DROP = {"Noun.name.csv", "Noun.org.csv"}
# Place names and other proper nouns: only the common ones (lower cost = more common).
COMMON_ONLY = {"Noun.place.csv", "Noun.proper.csv"}
MAX_COST = 8000


def is_kanji(c):
    return "㐀" <= c <= "鿿" or "々" <= c <= "〇" or "豈" <= c <= "﫿"


def word_class(pos1, pos2, surface, conj_type):
    """The one-letter class romanize/ja_dict.rs reads (see the docstring)."""
    if (pos1 == "助動詞" and conj_type == "特殊・タ") or (pos1 == "助詞" and pos2 == "接続助詞" and surface in ("て", "で")):
        return "T"
    if pos1 == "助詞":
        return "P"
    if pos1 == "助動詞":
        return "A"
    if (pos1 in ("動詞", "形容詞") and pos2 in ("非自立", "接尾")) or (pos1 == "名詞" and pos2 == "接尾"):
        return "J"
    if pos1 == "接頭詞":
        return "F"
    if pos1 == "記号":
        return "S"
    return "N"


def csv_field(s):
    return '"' + s.replace('"', '""') + '"' if ("," in s or '"' in s) else s


def source_tarball(path):
    """The tarball's bytes (from `path`, or downloaded), checked against SHA256."""
    if path:
        with open(path, "rb") as f:
            data = f.read()
    else:
        req = urllib.request.Request(SOURCE, headers={"User-Agent": "oyen-poni-make-ja-dict"})
        with urllib.request.urlopen(req, timeout=120) as r:
            data = r.read()
    got = hashlib.sha256(data).hexdigest()
    if got != SHA256:
        sys.exit(f"mecab-ipadic tarball SHA-256 is {got}, expected {SHA256}")
    return data


def read_source(data):
    """{file name: text} for the dictionary's CSV and .def files (EUC-JP in the tarball)."""
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member in tar.getmembers():
            name = os.path.basename(member.name)
            if member.isfile() and (name.endswith(".csv") or name in ("matrix.def", "char.def", "unk.def")):
                files[name] = tar.extractfile(member).read().decode("euc_jp")
    return files


def write_inputs(files, out_dir):
    """Write the trimmed lex.csv and unk.def, plus matrix.def and char.def, to `out_dir`."""
    kept = 0
    with open(os.path.join(out_dir, "lex.csv"), "w", encoding="utf-8", newline="\n") as w:
        for name in sorted(files):
            if not name.endswith(".csv") or name in DROP:
                continue
            for row in csv.reader(io.StringIO(files[name])):
                surface, left, right, cost, pos1, pos2 = row[:6]
                if name in COMMON_ONLY and int(cost) >= MAX_COST:
                    continue
                feature = word_class(pos1, pos2, surface, row[8] if len(row) > 8 else "")
                if len(row) > 11 and any(is_kanji(c) for c in surface):
                    feature += "," + row[11]
                w.write(f"{csv_field(surface)},{left},{right},{cost},{feature}\n")
                kept += 1
    with open(os.path.join(out_dir, "unk.def"), "w", encoding="utf-8", newline="\n") as w:
        for row in csv.reader(io.StringIO(files["unk.def"])):
            if row:
                w.write(f"{row[0]},{row[1]},{row[2]},{row[3]},{word_class(row[4], row[5], '', '')}\n")
    for name in ("matrix.def", "char.def"):
        with open(os.path.join(out_dir, name), "w", encoding="utf-8", newline="\n") as w:
            w.write(files[name])
    return kept


def compile_dict(in_dir, out_path):
    """Run tools/ja-dict (vibrato + zstd) on the files in `in_dir`."""
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(ROOT, "target", "ja-dict"))
    manifest = os.path.join(ROOT, "tools", "ja-dict", "Cargo.toml")
    args = [os.path.join(in_dir, n) for n in ("lex.csv", "matrix.def", "char.def", "unk.def")]
    subprocess.run(["cargo", "run", "--quiet", "--release", "--locked", "--manifest-path", manifest, "--", *args, out_path], env=env, check=True)


def main():
    args = sys.argv[1:]
    check = "--check" in args
    src = None
    if "--src" in args:
        i = args.index("--src")
        if i + 1 >= len(args):
            sys.exit(__doc__)
        src = args[i + 1]
    if any(a.startswith("-") and a not in ("--check", "--src") for a in args):
        sys.exit(__doc__)
    files = read_source(source_tarball(src))
    with tempfile.TemporaryDirectory() as tmp:
        kept = write_inputs(files, tmp)
        out = os.path.join(tmp, "ja-readings.dic.zst") if check else OUT
        compile_dict(tmp, out)
        with open(out, "rb") as f:
            built = f.read()
        print(f"{kept} entries; {len(built)} bytes; sha256 {hashlib.sha256(built).hexdigest()}")
        if check:
            with open(OUT, "rb") as f:
                committed = f.read()
            if committed != built:
                sys.exit(f"{os.path.relpath(OUT, ROOT)} differs from what tools/make_ja_dict.py builds")
            print(f"{os.path.relpath(OUT, ROOT)} matches")


if __name__ == "__main__":
    main()
