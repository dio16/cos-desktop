import pathlib

p = pathlib.Path("C:/dev/cos-desktop/crates/cos-client/src/projection.rs")
lines = p.read_text(encoding="utf-8").splitlines()

start = next(
    i
    for i, line in enumerate(lines)
    if "An iteration listing a candidate it does not own" in line
)
# Find the closing of that block: the assert! after expect_err, then the closing braces.
end = next(i for i in range(start, len(lines)) if lines[i].strip() == "}" and "the refusal names" not in lines[i])
# The block ends with the `assert!(...);` whose message is "the refusal names the cause", so walk on.
end = next(
    i
    for i in range(start, len(lines))
    if lines[i].strip() == ");" and "owned by another iteration" in lines[i - 1]
)

replacement = """        // An iteration listing a candidate it does not own: the listed candidate **exists**, so the
        // existence rule passes and only the ownership rule can refuse this.
        let mut second_iteration = iteration.clone();
        second_iteration.iteration_ref =
            desktop_protocol::IterationId::new("fixture:iteration-2").expect("an id");
        second_iteration.index = 1;
        second_iteration.candidate_refs = vec![candidate.candidate_ref.clone()];
        let error = state
            .validate_effective_state(
                std::slice::from_ref(&Projection::Iteration(second_iteration)),
                &[],
            )
            .expect_err("a candidate owned by another iteration must be refused");
        assert!(
            format!("{error}").contains("owned by another iteration"),
            "the refusal names the cause: {error}"
        );"""

lines[start : end + 1] = replacement.splitlines()
p.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="")
print("ownership block replaced")
