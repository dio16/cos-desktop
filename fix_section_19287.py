import pathlib

p = pathlib.Path("C:/dev/cos-desktop/docs/development/IMPLEMENTATION_MASTER_PLAN.md")
t = p.read_text(encoding="utf-8")

old = "Every identity type goes through one validator, `validate_opaque`, and the module had **no test**."
if old in t:
    t = t.replace(
        old,
        "Every validated identity goes through one function, `validate_opaque`, and the module had **no test** - but **not every path** goes through it: `fixture()` builds its value directly, which this section records below.",
    )

old_tests = "- **Five tests:**"
new_tests = "- **Six tests:**"
assert t.count(old_tests) == 1
t = t.replace(old_tests, new_tests)

p.write_text(t, encoding="utf-8", newline="")
print("stale wording corrected")
