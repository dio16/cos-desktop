# Published MP-R0C native-stress records — sanitised, bounded

Six scenarios (keyboard, held-key, pointer, scroll, Ctrl+Tab surface navigation, combined)
were driven with **synthetic** PostMessage input through the approved S2B capture path on
the approved instrumented candidate. Each scenario is one 30 s capture scope and one
observation; these are not distributions.

## Provenance

- Source commit `bcce8167591f6f142232b3c5ca7c5c14c7da5cdf`; instrumented executable
  `sha256:850b73a029b80fa8ad49d3cc5ac38a5683df8ae962d0a66f3f4fb282c0141ccb` (approved),
  helper `sha256:4d64f7c3ab8074af0d507c51b70e1a2757de47763c0bf088390df9e4d803b3ec`,
  fixture `sha256:2e33ed061c07bd97b8e4c4f823c028987e9415e83858878270489fb5b8c18c52`,
  GPUI pin `395def7a067b32ed598fcb3f048969ada9b54942`, feature `r0-s2b-instrumentation`,
  Windows 11 build 26300, Ryzen 7 3700X, RTX 2080 SUPER.
- Each run went through `s2b_capture.py` (approved S2A launcher + passive S2B capture +
  replay); the per-scenario execution identity is in the table.
- Input is synthetic: WM_KEYDOWN/WM_KEYUP, WM_MOUSEMOVE, WM_LBUTTONDOWN/UP and
  WM_MOUSEWHEEL posted to the app window, not a real user session. The window was closed by
  an operator WM_CLOSE at 40 s.

## Scenarios (requested vs achieved)

| scenario | requested | achieved | messages | retained/eligible | dropped/unassociated | retained kinds | draw gaps | coalesced records | settle after last input | execution |
|---|---|---:|---|---|---|---|---|---|---|
| `keyboard` | 20 Hz | key 20.039 Hz | key 500 | 256/2420 | 2164/968 | input 102, dirty 52, draw 51, submit 51, surface-mutation 0, notify 0 | max 27.022 ms / 51 intervals | 0/102 (max 1) | n/a (inputs incomplete) | `a4fc67ef08f947f39e28ef2086fd0675` |
| `held-key` | 30 Hz | held-keydown 30.043 Hz | held-keydown 750 | 256/2904 | 2648/728 | input 64, dirty 64, draw 64, submit 64, surface-mutation 0, notify 0 | max 14.519 ms / 64 intervals | 0/128 (max 1) | n/a (inputs incomplete) | `efa2d133b04a469a8a453feb8a673a97` |
| `pointer` | 30 Hz | move 30.037 Hz, click 1.541 Hz | move 750, click 38 | 256/3023 | 2767/838 | input 71, dirty 62, draw 62, submit 61, surface-mutation 0, notify 0 | max 18.824 ms / 62 intervals | 123/123 (max 6) | n/a (inputs incomplete) | `17031106ba3d420598c0e1b574ab1239` |
| `scroll` | 10 Hz | wheel 10.039 Hz | wheel 250 | 256/482 | 226/241 | input 128, dirty 0, draw 0, submit 0, surface-mutation 0, notify 0 | no draw intervals | 0/0 (max 0) | n/a (inputs incomplete) | `e0499a69a36c4a05bd46e9b008818ec3` |
| `navigation` | 2 Hz | ctrl-tab 2.041 Hz | ctrl-tab 50 | 240/240 | 0/96 | input 96, dirty 48, draw 48, submit 48, surface-mutation 0, notify 0 | max 474.867 ms / 48 intervals | 0/96 (max 1) | n/a (inputs incomplete) | `c9aed2ff23ea4dbf869f87a6729117d2` |
| `combined` | 55 Hz | key 20.04 Hz, move 30.041 Hz, wheel 5.041 Hz | key 500, move 750, wheel 125 | 256/2962 | 2706/1898 | input 167, dirty 3, draw 85, submit 0, surface-mutation 0, notify 0 | max 2.208 ms / 85 intervals | 42/85 (max 5) | n/a (inputs incomplete) | `e3a26ee9b0c74a31af219c8584cf77a5` |

Scenario notes: keyboard/held-key/pointer/combined hit the 256-record retention cap
(thousands of entries dropped), so retained counts are not event totals; `input-unassociated`
is large because the pin's InputTiming carries no window ID. `scroll` produced no
dirty/draw/submission records (the wheel did not invalidate the fixture surface) but 128
internal accounting records. `navigation` (Ctrl+Tab) produced input/draw/submission records
but no `local-surface-mutation`/`local-notify` record, so surface navigation could not be
confirmed with this fixture; its ~475 ms draw gaps match the 500 ms inter-action spacing and
are not stall evidence. `combined` retained no submission record under the cap.

## Queue, cancellation and restoration

- **Backlog evidence:** the journal records 256 retained of up to 428 eligible entries under
  its fixed retention cap, with `retention_dropped` and `excluded` counters in each capture.
  Queue depth is **NOT_MEASURED**: the build has no queue-depth instrumentation.
- **Aggregation/coalescing:** the "coalesced records" column counts retained draw/submission
  records that carry more than one invalidation. It shows that **multiple invalidations were
  aggregated into a single draw**; it does **not** prove that an individual change was discarded,
  which change replaced which, or any queue depth. Scenarios whose column reads 0 simply recorded
  a single invalidation per draw.
- **Cancellation:** no cancellable in-flight work can be produced by this fixture workload and
  the build exposes no cancellation counter; cancellation is **NOT_MEASURED**.
- **Settle:** the settle column is the interval between the last retained input record and the
  last retained dirty/draw/submission record. UI state restoration after input stops is
  **NOT_MEASURED** (no AX/visual probe in this path); only journal quiescence is observed.

## Stall attribution

The only attributable evidence is internal journal timing: gaps between consecutive retained
draw intervals (table) and submission intervals (per-result `draw-gap` metric). Because no
present/displayed/scanout endpoint exists on this host, the §11.1 visible-frame stall gate is
**`UNVERIFIED`** for every scenario: a journal gap is not a visible-frame stall, and this
record does not attribute any gap to message servicing, frame processing, submission, Present
or composition. No cause is inferred. The stall-gate metric in each result carries the closed
reason.

## NOT_MEASURED / UNVERIFIED

- `NOT_MEASURED`: present-call, presented-frame, input-to-photon, scanout, gpu-submission
  (no probe on this host); frame-requested/frame-processing-started (app-side only at the pin);
  paint sub-split, VRAM high-water, process CPU/working-set, queue depth, cancellation,
  restoration, and all visual/AX state.
- `UNVERIFIED`: the §11.1 visible-frame stall gate (no present endpoint), per scenario.

## Volume

One repetition per scenario (6 runs). §10 asks for ≥30 interaction repeats and ≥5 × 60 s runs;
this record is **one observation per scenario**, not a distribution. A repeatable
self-terminating workload driver still needs: scripted per-scenario action traces with
achieved-cadence recording, deterministic in-process close/exit, longer declared scopes, a
repetition harness with recorded order, and aggregation/estimators (the S1 contract rejects
percentile claims).

## Replay

```
python -B apps/desktop/devtools/r0/s2b_validate.py capture --capture capture-run/<scenario>/s2b-capture.json
python -B apps/desktop/devtools/r0/validate.py pair r0c/<scenario>-manifest.json r0c/<scenario>-result.json
sha256sum -c SHA256SUMS
```

Files are bounded records only: no credentials, no user content, no absolute local paths.

## Digest correspondence

`SHA256SUMS` records the **published LF** digests. The table adds the digest each file has when CRLF is restored, which is the form in which several records were produced, so a reviewer can match either form.

| file | published (LF) | CRLF-restored |
|---|---|---|
| `README.md` | `f59ed3aba1601cfcaa5aaaeaaca4fc54d7e4db3059754169ffc2c54582fa6387` | `ba53349e8e163842b90a1bf4a260d71e4e8ee3c853cd3bbe565f8793a18ba94d` |
| `capture-run/combined/ledger.json` | `c1c5e1dbb582c42fa73e5121836c8dd259f8679ee0192ad32470de7b68de5c72` | `9d47aa4b53ec5184d64676007b4b40c5e7fd7a572e5109516e03d070c4527562` |
| `capture-run/combined/s2b-capture.json` | `05ca2515a40d910bf76b1f6811dfda227bd58bc6684be7bf5896b981ba69bc7c` | `0b5747a81b0e833b4a587bdd643ca1908525236071be76e4cd7e852c875eaa4f` |
| `capture-run/combined/s2b-observation.json` | `18a81eb14bfc30114281f6db775ecf01c294a6f93b1a18d75ce3db8b8fde7ee2` | `dcd6df37a143c96237fed9f8d0b9079405c1146ffc0e2bd79b352678a083d410` |
| `capture-run/held-key/ledger.json` | `9e70b3a9160e4e7b3765534e2053f79b1950584b39dd75aebaf475339db4cebf` | `060eb161bcbbf9ba872c40cb6589bb210022467bc62e73ceed59beb478de0a56` |
| `capture-run/held-key/s2b-capture.json` | `6b227859668a29ae17f0f0ef9d00d84539e6ab3fc268fee082341e4ac5f0d698` | `5bc89f3925bbd8fed5d9340de0181365dc575897e2d7d30c7271acef32c96a1f` |
| `capture-run/held-key/s2b-observation.json` | `a530ebbe3b66c67eb9240653f2a774562a8a6f29b867e73fb43bf397139491d8` | `2db3a02bf13b8eea990b2b00b24ecee85e86b85311bc302b8f796ff88df6bf7d` |
| `capture-run/keyboard/ledger.json` | `72310ac34055d56a09811554ac3370edd083f7f4e41012b3d8bd31d19e17c185` | `24a7e234bc369f672974541d663084f168db78b699cccf1caee7055c19eb2a2e` |
| `capture-run/keyboard/s2b-capture.json` | `9041fc1516558f06f153648b4bc6f432dce7802d256d3ee6f5b7bf458d0855da` | `d79dace8f69cc4bcd89d88433476921301bccfa544a9dbf7c7111e2502f065c7` |
| `capture-run/keyboard/s2b-observation.json` | `c21b6fe7d347e77d252431bd83539a188d887b4999258c0a0a80e2f349c6eb99` | `eb29989b4cbb118c06a7d0a349d43c656cc3200c9969b9afd0c14b1c4f20a1d4` |
| `capture-run/navigation/ledger.json` | `ddb0a4195a9d726b3deeaa150451054468ef6c244381fb65997cae1b38a69714` | `1d73fac25d03ea7f1ba6630b200c267381e693e7396524b7d047229543e25e05` |
| `capture-run/navigation/s2b-capture.json` | `dbcdec0d633c4086e94df0bb6bcbb38b3b774ba62658b5a38d27c78a94f7f857` | `e7fd499af3cd1a201b98963f4b27470d205e2aac4279db97930d2eb7df7e71ef` |
| `capture-run/navigation/s2b-observation.json` | `c379e43fe69724793a9aa3292ef1272667ff6f8a29abeb5384db070978d9ddfb` | `1c715aa6aed0b4580bca1c0c527df65d505eee255983314844277b78472d7b4c` |
| `capture-run/pointer/ledger.json` | `7e1444c1ab1a8e38e7d55fa2cdc403b82d8054c9c9cdd6c3c4eb46de146e91c3` | `57d6924fc98483d7aad0185ef4c04b9228f8de1d8ef12a9b1456fa059d7a12ae` |
| `capture-run/pointer/s2b-capture.json` | `1bbed745958ccf2e33c7c7e4997a8ce0c76ded37a08445d6892e48bead41794d` | `f5859c8d3ccd071b0a8c2b1dd6b55d871c80c34823344fbe091d3c84cc8a1e08` |
| `capture-run/pointer/s2b-observation.json` | `a2ae0da1ce8586473c1cd1122bc557a0eaaa9f9a2a87fa9b4a7eaeaefd42129a` | `ea6f0d6c548525054ad655cf513cfdcf814454ac18486e89b5681f515bcd0972` |
| `capture-run/scroll/ledger.json` | `88f616ff727f3c6485cfacc377e35acdcc50b96701e1611ee9ead548f27d6a69` | `0bd93509428f9b7088c98bef0851fc298ffe42155e389374d45e482b89a3c2e8` |
| `capture-run/scroll/s2b-capture.json` | `56c8bb2e186d39dd53b966c806de2d6becf3dbd6d06c94f44cce249be8c49dfc` | `dea8342b461df12edbdc7fbf6717a73c3acee3125d6ad37ac04a23dcf07340e0` |
| `capture-run/scroll/s2b-observation.json` | `652c1ab64cdc0a79d0d630fc92d89b106782930685b6f768617356a6ea054a1d` | `e0cf2227d167c6e718bb450eaf3ab91c8d175e684a41c35cf20456965e5c805c` |
| `r0c/combined-manifest.json` | `d7709974671fb8356f154a7b855a6acd889c325ec17d89ea28197c26142c2fbd` | `86c96d5caf086f5efecfd6ff3d6136650432602754df63a10a46d7baacfd55f5` |
| `r0c/combined-result.json` | `a300c6b459adfc957bef6266d105b838aad8d7ce7f782bbc512e2e65d4ec5cc6` | `4ca2847c9d71d2f12055df5706f0fd8c3f496a7a98c8832e592f89bd08dc6202` |
| `r0c/held-key-manifest.json` | `6e69702ca27ccad3cee7ed71f145309ca84b71a5a8d306472761db44d9dd8a26` | `e4aaba4385d2fae0d8654461af68a89dcf1e5a54e5e2f3295575947f43728735` |
| `r0c/held-key-result.json` | `12b448a9b8afaf912fd227b0c4e2f5684dbb47a08af16b832d0b2eaebbc16a7b` | `74345d4320837ef8801999ef3e7d5ab82e4633b49b9841c06ad37172d925ba40` |
| `r0c/keyboard-manifest.json` | `641d2de586f23dfd5d85fcfde73a9f756d5b178cf7871d3d453fa7f2e4cbb0c7` | `982ff665d53fd40ff64547f8f2af5a629a83c73e44d896844e3bdcee4289a7b9` |
| `r0c/keyboard-result.json` | `39fd4afd8f19a024c8fa7f785a00f427f209d3d92aa10fe31ec90942cd8c78be` | `fc99834cbe280af3d497d198bc7f3e8dba6577580c5d4b8ec6f7d5d2b6b99551` |
| `r0c/navigation-manifest.json` | `3ba4efd1ce6d0f31e36e3950605f573d230d4bd985b06aa23f66aa88f4f5860c` | `15ad4f3fe3f19ffb0286d28a4fad4ecc1437d212b3bc53712fb87fbbd2fba9ca` |
| `r0c/navigation-result.json` | `dd433451bbcb024c02efaedfcd424e4309140a604449dba0edcef0ebb9292fc6` | `5c1a51f6025cbfc9432a6b76dac607ad7f6e75e5f1b835556fe8d5a36881fab0` |
| `r0c/pointer-manifest.json` | `6857885ea290caaa436cf09691fecb0fb866a738e4e346c0e85a316ba60edded` | `4592bd552801a979a9806d657699558f49751aae729b286caab501362baf8e33` |
| `r0c/pointer-result.json` | `a24fc99cd6b3fa7f38c299c2c2126a32da0b034c5e3874d8ddc7d25a4a54da5f` | `0e39e012d4d555e2f7c371010a8c223f1c61426142fb4b21b6f83b70265058ef` |
| `r0c/scroll-manifest.json` | `9830ca5a5a0508d890ff2945fce338cd7de9f3fb8e65d053c4ac47a1ef4f355e` | `ba065373dda28b8ac7c8257fba4dd06fb2197910d2c75c22800d53e559dafa11` |
| `r0c/scroll-result.json` | `ad584c382b9f0d1bfc960e13ea0f8d1ea8aebd44dac09197ab6f7b9c3e3b2802` | `a9fc97c6a2191d6455580efa2b0f977f7774c6b245af86d82499f7ad80f5219b` |
| `raw/combined-driver.json` | `bc4832a1324df259694adc35c7cab116cbc30fbc7f6ac40937f3f8ac0b181c51` | `0dc311c754d4c10964da2d30444d1eb38f85bf0b05fbfdd086895e189a4750e4` |
| `raw/held-key-driver.json` | `9663b7c40bd2a2438c8affdd5a71f50529ad19f6c020da25a7af382adfe44c5c` | `0361385b25dfc9115bb9070fda0e64f807994423601fef74e2e471b5e979a72a` |
| `raw/keyboard-driver.json` | `76c978e5e7d6ef8cadd5fb222afa87329e18e67d63b9743246fdf2843e57e2a9` | `9cf6c279f7f108bf6a83292c1003694a140d2430843758e759c88541a2ec451a` |
| `raw/navigation-driver.json` | `019ec4d24000e2b7a947de94d3594f65ce6957900623c178d9bd62be4b6f08ad` | `0f4c6308a7e08aff8668a174ab76315a2f2122e948f636d5c8463fc659df207c` |
| `raw/pointer-driver.json` | `e036cddc25c2f37f7725a22dab6a7553ae8b08ac72e9e79b703d85057d852748` | `dba6b9998b35f67c41a39aee74c9866fc80a489b4ee93856962d465c511c8f5c` |
| `raw/scroll-driver.json` | `49b434e610df67ed6c32d486e4ed45b33b37872cd7fa9b8911447d07dfe499dc` | `b354aac7c57b56a5ca7bbfa0209fe0650899e98791dd0e15b500cf3f2ac8f180` |

No measurement, record field or recorded digest was edited for this table; it only states both digest forms.
