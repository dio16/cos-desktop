import pathlib

p = pathlib.Path("C:/dev/cos-desktop/docs/development/IMPLEMENTATION_MASTER_PLAN.md")
t = p.read_text(encoding="utf-8")

old = "- **Proven by mutation, six ways:**"
new = "- **Proven by mutation, ten ways:**"
assert t.count(old) == 1
t = t.replace(old, new)

old2 = "- **Verified:** `cargo test -p cos-client projection::` 10 passed;"
new2 = ("- **Verified:** `cargo test -p cos-client projection::` **11 passed** - the ten in this module plus the "
        "recorded-fixture test that `projection::` also matches; the narrower `projection::integrity_tests` filter "
        "runs the ten described here;")
assert t.count(old2) == 1
p.write_text(t.replace(old2, new2), encoding="utf-8", newline="")
print("counts corrected")
