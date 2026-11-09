# ComfyUI generation plane break — `comfy-aimdo` / `comfy-kitchen` repair (2026-11-10)

The MP-ACC1 generation plane stopped starting ComfyUI after a completed generation. Every
restart failed while importing `comfy_aimdo.storage`. This record captures the diagnosis, the
two small package repairs, the bounded startup verification, and the exact rollback.

- **Classification:** environment break — ComfyUI source was auto-updated by ComfyUI-Manager
  without installing the matching core requirements. **Repaired.**
- **Scope:** the external generation stack under `C:\AI\ComfyUI`, not the product repos.
  No ComfyUI git checkout, model file, custom node, Ollama setting or other package was changed.
- **Not verified here:** a real MP-ACC1 generation (only a bounded startup was run; see
  [Residuals](#residuals)).

## Observed state at failure

| Item | Value |
| --- | --- |
| ComfyUI portable root | `C:\AI\ComfyUI\ComfyUI_windows_portable` |
| Python | embedded `python_embeded\python.exe`, 3.13.12 |
| PyTorch | 2.12.0+cu130, NVIDIA GeForce RTX 2080 SUPER (8192 MB) |
| ComfyUI source | git `master` at `fca60fe70e5b84c5c6b058263653d15cb3401e44` (`v0.44.0-33-gfca60fe70`); `comfyui_version.py` = `0.44.0` |
| Installed `comfy-aimdo` | **0.5.3** |
| Installed `comfy-kitchen` | **0.2.33** |
| `requirements.txt` pins | `comfy-aimdo==0.6.0`, `comfy-kitchen==0.2.41` |
| Launcher | `scripts\start_services.ps1` (repo `C:\AI\orchestrator`) starts `run_8gb.bat` with `--lowvram --reserve-vram 1.5 --cache-lru 10 --preview-method taesd --use-pytorch-cross-attention` on port 8188 |
| Listener at diagnosis time | none; three failed starts in `ComfyUI\user\comfyui*.log` (23:49, 00:05, 00:14) |

The user report refers to ComfyUI 0.35.1. That was the checkout before the update described
below; at failure time the observed checkout was `0.44.0` (`fca60fe70`).

## Symptom — exact traceback

All three restart attempts failed with the same import chain (`ComfyUI\user\comfyui.log`,
rollover `comfyui.prev2.log`):

```text
Traceback (most recent call last):
  File "C:\AI\ComfyUI\ComfyUI_windows_portable\ComfyUI\main.py", line 266, in <module>
    import comfy.utils
  File "C:\AI\ComfyUI\ComfyUI_windows_portable\ComfyUI\comfy\utils.py", line 26, in <module>
    import comfy.storage
  File "C:\AI\ComfyUI\ComfyUI_windows_portable\ComfyUI\comfy\storage.py", line 7, in <module>
    import comfy_aimdo.storage
ModuleNotFoundError: No module named 'comfy_aimdo.storage'
```

Reproduced with the embedded interpreter from `C:\AI\ComfyUI\ComfyUI_windows_portable\ComfyUI`:

```text
> ..\python_embeded\python.exe -c "import comfy_aimdo.storage"
ModuleNotFoundError: No module named 'comfy_aimdo.storage'
```

The installed `site-packages\comfy_aimdo` (0.5.3) contains `control.py`, `host_buffer.py`,
`malloc_graph.py`, `model_mmap.py`, `model_vbar.py`, `torch.py`, `vram_buffer.py` and the two
DLLs, but **no `storage.py`**. `comfy_aimdo.storage.fast_disk()` is the only
`comfy_aimdo.storage` API used by `comfy/storage.py`, from `fast_storage()`.

After repairing `comfy-aimdo`, startup advanced one step and failed on a second stale package
captured in the verification log:

```text
  File "...\ComfyUI\execution.py", line 18, in <module>
    import comfy.model_patcher
  ...
  File "...\ComfyUI\comfy\text_encoders\llama.py", line 9, in <module>
    from comfy_kitchen.int8_decode import Int8DecodeCache
ModuleNotFoundError: No module named 'comfy_kitchen.int8_decode'
```

`comfy_kitchen` 0.2.33 has no `int8_decode` module; it is present in 0.2.41.

## Root cause

ComfyUI-Manager updated the ComfyUI **source** from the pinned v0.35.1 checkout to
`master` but did not install the **core requirements** that changed with it. Evidence:

- `ComfyUI\user\__manager\config.ini` has `update_policy = nightly-comfyui`; the file and the
  Manager snapshot directory were last written at `2026-11-09 23:32`.
- `git reflog` in the ComfyUI checkout records the update at `2026-11-09 23:32:45–54`:
  `checkout: moving from master to v0.44.2`,
  `checkout: moving from v0.44.2 to master`, then
  `pull -v -- origin: Fast-forward` to `fca60fe70`.
- `git diff v0.35.1..master -- requirements.txt` changes exactly the core packages that stayed
  old: `comfyui-frontend-package 1.51.10→1.58.5`, `comfyui-workflow-templates 0.11.59→0.11.103`,
  `comfyui-embedded-docs 0.5.11→0.5.16`, `comfy-kitchen 0.2.33→0.2.41`,
  `comfy-aimdo 0.5.3→0.6.0` (plus `torchaudio` removal and `Pillow>=10.1.0`).
- `site-packages` shows the Manager dependency installer did run at `23:49` (it installed
  custom-node packages such as `albumentations`, `numkong`, `stringzilla`), but
  `site-packages\comfy_aimdo-0.5.3.dist-info` remained untouched. The source update and the
  dependency install are separate Manager paths; only the custom-node path ran.
- Manager rendered the failure differently from a missing custom node, so the log shows the
  bare Python traceback rather than a Manager installation error.

The dependency pins themselves were not ambiguous: the checked-out source's own
`requirements.txt` names `comfy-aimdo==0.6.0` and `comfy-kitchen==0.2.41`, and the wheels for
those exact versions contain `comfy_aimdo/storage.py` and `comfy_kitchen/int8_decode.py`.

## Repair

Only the two directly implicated core packages were changed, using the exact pinned wheels.
Both installs used `--no-deps` (neither wheel has a mandatory runtime dependency; the only
`Requires-Dist` entries in 0.2.41 are extras). No package was upgraded in place as a side effect.

Pre-repair backup (kept outside the git repo, under `C:\AI\ComfyUI\ComfyUI_windows_portable\backups`):

| Artifact | Size | SHA-256 |
| --- | --- | --- |
| `comfy_aimdo-0.5.3-backup-20261110-0130.zip` (package + dist-info) | 297,829 B | `caf86a99085b4909afbc28187f0af53a91566b0e3262adc121be3fd685ab8392` |
| `comfy_aimdo-0.5.3-cp39-abi3-win_amd64.whl` (rollback wheel) | 274,405 B | `551f991e6a8a881579906e231e68935fe58c80165eabb4ef7e7e76a4c70bebde` |

Commands actually run (from the ComfyUI portable root):

```bat
cd /d C:\AI\ComfyUI\ComfyUI_windows_portable
:: 1) comfy-aimdo 0.5.3 -> 0.6.0  (wheel 284,475 B, sha256 45f821b5828c4b38bcac6553f2250b56e6db87bd65334ed37eb938985d809a3c)
python_embeded\python.exe -m pip install --no-deps --force-reinstall --no-cache-dir ^
  %TEMP%\comfy_aimdo_060\comfy_aimdo-0.6.0-cp39-abi3-win_amd64.whl
:: 2) comfy-kitchen 0.2.33 -> 0.2.41 (wheel 65,283,137 B, sha256 cbe8f7fd9aa42e2af01a31d235fd4de59eb429ecc5a58287eed0975195b68707)
python_embeded\python.exe -m pip install --no-deps --force-reinstall --no-cache-dir ^
  %TEMP%\comfy_aimdo_060\comfy_kitchen-0.2.41-cp312-abi3-win_amd64.whl
```

Result: `pip show` reports `comfy-aimdo 0.6.0` and `comfy-kitchen 0.2.41`. The checkouts, the
other core packages, custom nodes and model files were not modified.

## Verification

Checked with the embedded interpreter:

```text
> python_embeded\python.exe -c "import comfy_aimdo.storage as s; print(s.__file__, hasattr(s,'fast_disk'))"
...\site-packages\comfy_aimdo\storage.py True
> python_embeded\python.exe -c "import comfy.storage, comfy.utils, comfy_kitchen.int8_decode, comfy.text_encoders.llama; print('OK')"
OK
```

Bounded startup, using the same flags `start_services.ps1` writes into `run_8gb.bat`
(no generation submitted):

```text
python_embeded\python.exe -s ComfyUI\main.py --windows-standalone-build --lowvram ^
  --reserve-vram 1.5 --cache-lru 10 --preview-method taesd --use-pytorch-cross-attention
```

- `GET http://127.0.0.1:8188/system_stats` returned **HTTP 200** after about 90 s.
- Response: `comfyui_version = 0.44.0`, `python_version = 3.13.12`, device
  `cuda:0 NVIDIA GeForce RTX 2080 SUPER : cudaMallocAsync` (8.59 GB reported total),
  `argv` exactly the documented flags above.
- Startup log confirms `ComfyUI version: 0.44.0`, `comfy-aimdo version: 0.6.0`,
  `comfy-kitchen version: 0.2.41`, `Starting server`, `To see the GUI go to: http://127.0.0.1:8188`.
- The `comfy_package_versions` block still warns about the intentionally untouched data/UI
  packages (`comfyui-frontend-package 1.51.10` vs 1.58.5, `comfyui-workflow-templates 0.11.59`
  vs 0.11.103, `comfyui-embedded-docs 0.5.11` vs 0.5.16). These are startup warnings, not
  import failures; see [Residuals](#residuals).

Cleanup: the verification ComfyUI process was stopped; `netstat` confirms **no listener on
8188** and no `ComfyUI\main.py` python process remains. Ollama was not touched;
`GET http://127.0.0.1:11434/api/tags` still returns **HTTP 200**.

## Rollback

Exact commands (from the ComfyUI portable root). The `comfy-aimdo` rollback is offline and
uses the retained wheel; the `comfy-kitchen` rollback uses the pinned PyPI version:

```bat
cd /d C:\AI\ComfyUI\ComfyUI_windows_portable
python_embeded\python.exe -m pip install --no-deps --force-reinstall ^
  backups\comfy_aimdo-0.5.3-cp39-abi3-win_amd64.whl
python_embeded\python.exe -m pip install --no-deps --force-reinstall comfy-kitchen==0.2.33
```

Important: rolling both packages back restores the **broken** state, because the current
`0.44.0` source requires 0.6.0 / 0.2.41. Do this only together with reverting the ComfyUI
checkout to v0.35.1, which this task did not do.

## Residuals

- **No generation was re-run.** Verification stopped at `/system_stats` HTTP 200 and a
  successful model-free startup, so MP-ACC1 end-to-end generation is not re-proven here.
- **UI/data packages are still behind the 0.44.0 pins** (`comfyui-frontend-package`,
  `comfyui-workflow-templates`, `comfyui-embedded-docs`). They do not block startup or the
  Python API execution path, but a workflow or UI feature that depends on a newer template/doc
  could still misbehave. The full catch-up is `pip install -r ComfyUI\requirements.txt` (or
  `update\update_comfyui.bat`, a much larger download); it was not performed because it is
  outside the failing import and would be a larger upgrade.
- **The trigger is still armed.** `ComfyUI-Manager` keeps `update_policy = nightly-comfyui`,
  so a future startup can update the source again without installing the new core pins. Not
  changed here (user configuration); the safe options are to lower/disable the update policy or
  to run the requirements install immediately after every source update.
- ComfyUI-Manager's `23:49` custom-node dependency install added/updated unrelated
  `site-packages` entries (`albumentationsx`, `numkong`, `stringzilla`, ...). Those were not
  changed by this repair and are not part of it.
- Downloaded wheels remain in `%TEMP%\comfy_aimdo_060` (about 64 MB); the rollback wheel and
  package zip remain in `C:\AI\ComfyUI\ComfyUI_windows_portable\backups`. Neither is referenced
  by the product repository.

## Evidence map

- Failure logs: `C:\AI\ComfyUI\ComfyUI_windows_portable\ComfyUI\user\comfyui.prev2.log`
  (and the two rollovers), plus the captured tracebacks above.
- Update evidence: `ComfyUI\.git` reflog, `ComfyUI\user\__manager\config.ini`,
  `ComfyUI\user\__manager\snapshots\2026-11-09_23-32-54_autosave.json`,
  `git diff v0.35.1..master -- requirements.txt`.
- Repair artifacts: `C:\AI\ComfyUI\ComfyUI_windows_portable\backups\`.
- Verification log: `%TEMP%\comfy_startup_verify.log` (first failure after the aimdo repair) and
  `%TEMP%\comfy_startup_verify2.log` (successful startup after the kitchen repair).
- No product code or configuration was changed by this record.
