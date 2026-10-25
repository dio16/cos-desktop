# Published S2B / R0B records — sanitised subset

This directory is the **publishable subset** of the locally retained records for the
MP-R0A-S2B + MP-R0B (G4) slice. The retained originals live outside version control; the
files here reproduce the retained originals as stated per file in the canonical
correspondence table near the end of this document: some match the published bytes directly,
some match only after restoring CRLF, one is the redacted transcript, and this README has
been edited. No measured field was withheld or altered.

## Provenance

- Source commit `bcce8167591f6f142232b3c5ca7c5c14c7da5cdf`; artifact rebuilt from a clean
  detached checkout (`cargo build --locked -p cos-desktop --features r0-s2b-instrumentation`).
- Instrumented executable `sha256:850b73a029b80fa8ad49d3cc5ac38a5683df8ae962d0a66f3f4fb282c0141ccb` (approved entry), helper
  `sha256:4d64f7c3ab8074af0d507c51b70e1a2757de47763c0bf088390df9e4d803b3ec`, helper source `sha256:5b585f8ea1adcea540fab97adff9449c0e2ea88af97e29d3f97d4bac522c8058`, fixture
  `sha256:2e33ed061c07bd97b8e4c4f823c028987e9415e83858878270489fb5b8c18c52`,
  GPUI pin `395def7a067b32ed598fcb3f048969ada9b54942`, feature
  `r0-s2b-instrumentation`, Windows 11 build 26300, Ryzen 7 3700X, RTX 2080 SUPER.
- Execution identity `34d0e01eee96466d83a9e6987bfc7ccb`, pid `45924`, run through
  `s2b_capture.py` (approved S2A launcher + passive S2B capture + replay); wrapper exit 0,
  `scope: SOURCE_BOUND_REPLAY`.
- **Input was synthetic**: 90 PostMessage events (mouse move/click, Tab keydown/keyup), not
  a real user session; the window was closed by an operator WM_CLOSE at 40.15 s.
- Disposition: `OBSERVED / RETAINED_SOURCE_OBSERVATION` (capture scope exactly 30 s,
  `cancelled=false`; decided by the corrected scope-based classifier).
- Retention: seen 516, eligible 428, retained 256, retention_dropped 172, excluded 62,
  pre_scope 26, input_unassociated 143, `journal_lost` 0. Endpoint kinds: input-dispatch 85,
  gpui-dirty 51, gpui-cpu-draw 69, gpui-platform-submission 51.

## Files and digests

The canonical per-file correspondence table near the end of this document is the single
authority on which published file equals which retained digest, and whether that equality
holds directly or only after restoring CRLF. The transcript differs in the redacted fields
listed below; this README itself was edited.

The R0B result embeds evidence digests. Its `raw/capture-run.json` entry
(`sha256:76b1ee27c8a3c3c6e922e029e60b9e11df3b7ca2171e73e64985e7b321612d27`) is the digest
of the **retained original** transcript, whose bytes are not published; the published
sanitised derivative therefore hashes differently
(`sha256:1bf53bc5b2e95c59bf7b3c23ca7e3a7b67f582b8185e44da684bdb63c6e13781`). That single
embedded pointer cannot be verified from this directory, by design. The other embedded
evidence digests resolve to the published files and verify, directly or after CRLF
restoration as the correspondence table states.

## Redactions

- `raw/capture-run.json` is the runner transcript. Redacted fields: `$.wrapper_command[0]`, `$.wrapper_command[2]`, `$.wrapper_command[4]`, `$.wrapper_command[6]`, `$.wrapper_command[8]`, `$.wrapper_command[10]`, `$.wrapper_command[12]`, `$.wrapper_command[14]`.
  Each was an absolute local path (interpreter, wrapper, manifest, executable, fixture,
  output directory, helper, profile directory) and was replaced with the closed placeholder
  `<REDACTED-LOCAL-PATH>`. The user name occurred only inside those paths, so every such value
  was replaced wholesale; no separate user-name field was published and no
  `<REDACTED-USER>` token is emitted.
- No measured record (capture, ledger, observation, completion, R0B manifest/result,
  consumer copies) contained a local path or user name, and their relation to the retained
  copies is exactly what the correspondence table records. No field was withheld because of
  redaction risk.
- Not present here by design: the retained-area README/SHA256SUMS (regenerated for this
  directory), anything outside the bounded record set, and the pre-sanitisation transcript.
  The retained original remains in the local ignored retained area; its bytes are not
  published (its digest is)
  because it contains local absolute paths and a user name.

## Replay

Replay needs the pre-fix consumer files in `consumer/` (they produced this capture; the
live tree has since changed) and the approved binaries by the digests in the provenance
section. Then:

```
python -B apps/desktop/devtools/r0/s2b_validate.py capture --capture capture-run/s2b-capture.json
python -B apps/desktop/devtools/r0/validate.py pair r0b/r0b-g4-manifest.json r0b/r0b-g4-result.json
sha256sum -c SHA256SUMS
```

## Recorded discrepancies (evidence not edited)

1. A first approved attempt (`f2a10352…`, capture `sha256:a97dbb0e…`) had a 27.489 s scope
   with `cancelled=true` and ledger elapsed 30,325 ms; the previous elapsed-based
   classifier reported `OBSERVED`, the corrected scope-based classifier reports
   `CANCELLED / CLOSED_BEFORE_DECLARED_DURATION`. Its bytes were overwritten in scratch and
   are not retained.
2. The historical bundle's `n02d` capture has a 28.996 s scope with `cancelled=true`
   (ledger 32,721 ms) and was historically reported `OBSERVED`; the corrected classifier
   reports `CANCELLED`. The archived bundle bytes are untouched; the replay test records
   the corrected classification.

This directory contains no credentials, no user content and no absolute local paths.


## Canonical digest correspondence (published LF vs retained original)

This replaces any earlier prose about "byte-identical" files. The table states, per
file, the digest of the **published bytes**, the digest after **restoring CRLF**,
and the digest of the **retained original**, with the relation spelled out.

| file | published (LF) sha256 | CRLF-restored sha256 | retained original sha256 | relation |
|---|---|---|---|---|
| `capture-run/build-record.json` | `c1f1c4926255b8cb58520ff2cf508385de220a61b0430945f9fda619c5c512ca` | `bd23fd303a8c0a613d58be9d8693f71e5c4ed827f67aea4248e24f18eb717248` | `bd23fd303a8c0a613d58be9d8693f71e5c4ed827f67aea4248e24f18eb717248` | identical after CRLF restoration |
| `capture-run/completion.json` | `fc0bbb9149ff7b364a7690dfaeb41d7dcf5449afe806c9c1632f0b4b61591dc5` | `81718744c19782e1e2fd91c756213a976d1af793052dd709f7b7faf698e23d82` | `fc0bbb9149ff7b364a7690dfaeb41d7dcf5449afe806c9c1632f0b4b61591dc5` | identical as published (LF) |
| `capture-run/endpoint-availability.json` | `619ca702323e64198f6d91150230fbf1b7982121b90cd6f49778c52670267303` | `baa42bf4ec4662396849c62ba5614b4c232fc79c310f5a2aa31bfd74e5d9c344` | `619ca702323e64198f6d91150230fbf1b7982121b90cd6f49778c52670267303` | identical as published (LF) |
| `capture-run/ledger.json` | `8f4ed492e5da5f5b75eb4ae8decbdb3598c6199c13a4c2d7ac27db93085a0af0` | `dfe3cb1af58a6a1a0d7127386d76a8a51168cca955b4521f7811cccd515388d5` | `8f4ed492e5da5f5b75eb4ae8decbdb3598c6199c13a4c2d7ac27db93085a0af0` | identical as published (LF) |
| `capture-run/result.json` | `11e518bceeff8ef63ed40a71abc8ab43aa2489a78ba7aa5d54d55a59da855446` | `1b1cad73104e5ab409b04dfde8a0d2a3b2a69895a50692327e93cc94266919ee` | `11e518bceeff8ef63ed40a71abc8ab43aa2489a78ba7aa5d54d55a59da855446` | identical as published (LF) |
| `capture-run/s2b-capture.json` | `2d8e3a6be3adf4eca5a6ba8d4b05e555b3cff2ea04a0bf18e6875a595542e472` | `2f35720182f5a545c45470441164d26e54a38efd0f58663ec42884805c77a851` | `2d8e3a6be3adf4eca5a6ba8d4b05e555b3cff2ea04a0bf18e6875a595542e472` | identical as published (LF) |
| `capture-run/s2b-observation.json` | `6051c6679b5f6563568bdbb042e335030b85420ff76de82f683d0861ce800737` | `a4004f71f04c5ac9e4bd9190e58aa3f6cb79971e1099de55e7cb4153ae1fa1de` | `6051c6679b5f6563568bdbb042e335030b85420ff76de82f683d0861ce800737` | identical as published (LF) |
| `capture-run/source-identity.json` | `eeeb7ccdf4f9c9458274e8fd9a3a363ba0326538a4879521ee2963fdb43cb202` | `9527741c824981d887d1977d296651bd345b5f761a2149b5dcc2faa5bccfdad4` | `eeeb7ccdf4f9c9458274e8fd9a3a363ba0326538a4879521ee2963fdb43cb202` | identical as published (LF) |
| `r0b/r0b-g4-manifest.json` | `bf636d2676a28d50a374aeaeb310db092fdad509cf48a9eb1f38b681bf478e84` | `eb51b2efe19ca78467eab5b59388092a5641700cda591f7ed9fe7803e10d14cc` | `eb51b2efe19ca78467eab5b59388092a5641700cda591f7ed9fe7803e10d14cc` | identical after CRLF restoration |
| `r0b/r0b-g4-result.json` | `2a2aa35e376a875e345c542ae27d6645b20ba989e900f7a57c2d637dd44c3b40` | `544655cf1be2369d1928dc269e4475c51b13a77ac0ab3af22ff0654d86bb9795` | `544655cf1be2369d1928dc269e4475c51b13a77ac0ab3af22ff0654d86bb9795` | identical after CRLF restoration |

**Summary:** 7 files are identical **as published** (they were already LF), 3 files
(`build-record.json`, the R0B manifest and the R0B result) are identical **after
CRLF restoration** (they were stored with CRLF), `raw/capture-run.json` is the
redacted transcript whose original digest is `76b1ee27…` (not published), and
`README.md` itself changed because this documentation was appended to it. The six
`consumer/` files are identical as published. `SHA256SUMS` is derived from the
published bytes only, so `sha256sum -c SHA256SUMS` verifies the published set
against itself; the three CRLF-restored matches above are checked separately and
are the reason the capture, manifest and result can be tied to the recorded values.
