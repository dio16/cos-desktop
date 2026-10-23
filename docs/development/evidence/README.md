# Historical evidence and publication boundary

## Historical records

Raw development evidence is not distributed in this public repository. The original records include machine-specific paths, environment captures, private review sessions, screenshots, build logs, approval records and independent verification receipts. They are retained unchanged in a separate private archive; their omission is intentional, not a successful verification result.

Links redirected here identify historical claims whose original supporting records are private. The surrounding document retains the recorded task, date, disposition and limitations. This page is not a replacement receipt, a new approval, or proof that a historical check can be reproduced from the public checkout. Original hashes and candidate commit IDs refer to the private archive, not to the publication snapshots. Do not reinterpret those identifiers as commits or verified bytes in this repository's new public history.

## Status and limitations

- Product/UX design, Desktop ownership, protocol boundaries and task acceptance requirements are unchanged by publication.
- Historical technical completion, independent verification, external review and Caller acceptance remain distinct. A summary does not grant any of them.
- The development branch's last recorded MP-R0A-S2B disposition is **HOLD**. Publication neither completes that task nor resumes paused implementation. The stable branch remains its earlier code baseline; development code is not promoted to it by this documentation change.
- Fixture observations do not establish real Runtime integration, Task survival, product UX approval or calibrated performance acceptance.
- Public consumers cannot independently validate private approvals, original receipts, private review sessions or historical source identities from these snapshots alone.

## Public and private material

The original Git history and raw evidence remain in the private predecessor repository. A local backup is also retained under Git's private metadata directory, which is outside every committed tree. Never push private backup refs, bundles or evidence into this repository, including through `git push --all` or `git push --mirror`.

Only this disclosure is tracked in the public evidence directory. Raw files may remain in an existing local checkout but are ignored. New fixtures required by product tests belong in the appropriate reviewed fixture directory, not in private development evidence.

Documentation uses placeholders such as `<desktop-root>`, `<runtime-root>`, `<user-home>` and `<evidence-root>` instead of the original workstation's paths. Substitute a suitable local location before using an illustrative command. These placeholders are not production endpoints or executable path values.

The private predecessor is a separate GitHub repository. A branch, tag or old commit inside a public repository cannot be made private independently.
