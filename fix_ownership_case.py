import pathlib

p = pathlib.Path("C:/dev/cos-desktop/crates/cos-client/src/projection.rs")
t = p.read_text(encoding="utf-8")

old_start = """        // An iteration listing a candidate it does not own. The candidate exists but belongs to a
        // different iteration, so this exercises the ownership rule rather than the existence one.
        let mut foreign = candidate.candidate_ref.clone();
        foreign.iteration = 1;
        let mut rewired = candidate.clone();
        rewired.iteration_ref = desktop_protocol::IterationId::new("fixture:iteration-2").expect("id");
        rewired.candidate_ref = foreign.clone();
        let mut second_iteration = iteration.clone();
        second_iteration.iteration_ref =
            desktop_protocol::IterationId::new("fixture:iteration-2").expect("an id");
        second_iteration.index = 1;
        second_iteration.candidate_refs = vec![foreign];
        let error = state
            .validate_effective_state(
                &[
                    Projection::Iteration(second_iteration),
                    Projection::Candidate(rewired),
                ],
                &[],
            )
            .expect_err("a candidate owned by another iteration must be refused");"""

old_end = """        assert!(
            format!("{error}").contains("owned by another iteration"),
            "the refusal names the cause: {error}"
        );"""

start = t.find(old_start)
assert start != -1
end = t.find(old_end, start)
assert end != -1
end += len(old_end)

new = """        // An iteration listing a candidate it does not own: the listed candidate **exists**, so the
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

p.write_text(t[:start] + new + t[end:], encoding="utf-8", newline="")
print("ownership case rebuilt")
