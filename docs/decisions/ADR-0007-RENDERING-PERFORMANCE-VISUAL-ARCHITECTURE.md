# ADR-0007: Bounded GPUI presentation and artifact-specific rendering

Status: **Accepted** technical architecture direction on 2026-10-08, following the user's request to ratify the design. **Observed** measurements are in the [evidence report](../development/TERN_BINARY_RENDERING_ANALYSIS.md). Numeric performance SLOs, exact visual tokens, video interop and any GPUI pin change remain **Proposed** or **Deferred** where explicitly marked. Accepted design is not implemented behavior or implementation authorization.

## Context

The current source builds a real Windows GPUI fixture shell, but Artifact, Compare and Flow renderers are not implemented. Eager collection construction and root-owned presentation state are source-observed constraints; they are not evidence that the GPUI GPU renderer is intrinsically unsuitable. Tern's installed binary provides a reference for bounded updates and native surfaces, not a product/UX specification. Temporary native workloads and decoder probes must not be reported as production COS features.

The [Product/UX canon](../architecture/COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md) remains the sole product authority. Existing repository, Domain, protocol, process and dependency boundaries remain unchanged. No Runtime repository changes, product source changes, dependency changes or push are authorized here.

## Decision

1. **KEEP** GPUI revision `395def7a067b32ed598fcb3f048969ada9b54942` for the shell, text, layout, input, focus and ordinary controls. No unconditional UPDATE, fork or full-stack replacement. Review the known stale-atlas-reference risk before image eviction is enabled; a reproduced defect can justify a separately reviewed narrow update/patch.
2. Use app-core-mediated immutable projection handles/change sets, stable scoped identity, background view derivation and entity-local presentation ownership. Retain canonical protocol ordering and atomic snapshot/subscription. Avoid a universal virtual-DOM/diff layer, crate merger or duplicated Domain model.
3. Mount/layout/shape long collections in proportion to visible rows plus declared overscan, not logical collection length. Fixed-height `uniform_list` is one verified mechanism, not a substitute for variable-height, keyboard, screen-reader or stable-anchor validation.
4. Choose **A first, B as the specialized architecture**: ordinary GPUI elements/entities, with narrowly scoped image/graph drawing and resource policies only where capability or measured workload requires them. Images use separate thumbnail, preview and visible-detail representations; comparison shares same-key decode/upload resources, not pane transforms. Graph inspection uses background revision-tagged layout, spatial queries, culling, level of detail and cached batched geometry.
5. Decode, search, filesystem/network, graph layout and bulk projection derivation stay off the UI thread. All queues, staging, caches and decoder scratch are byte/concurrency bounded and cancellation/generation aware. Whole-image `img()` is not the large-artifact architecture; oversized sources require bounded conversion/streaming/tiled representations or an explicit unavailable detail mode.
6. Idle rendering is demand driven. No decorative perpetual animation, periodic full repaint or default full-window blur. Instrument requests, skipped frames and thread work separately: no present events does not mean zero CPU cost. Any backend scheduling change requires reproduced attribution, not a speculative patch.
7. Treat typography, geometry, color, icons, depth and motion as one semantic token system, validated together with frame/resource cost, Japanese fallback, accessibility and per-monitor DPI. Do not embed proprietary Tern fonts/assets or scatter RGB/Unicode decoration across widgets. Exact candidates in the companion remain Proposed until native validation.
8. Keep video conditional. Successful FFmpeg hardware decoding does not prove Media Foundation compatibility, displayed playback, GPUI import, zero-copy or synchronized compare. The pinned Windows `draw_surfaces` no-op is an explicit integration gap. Select an end-to-end path only after copy/format/lifetime/PTS/device-generation measurements.

## Evidence and alternatives

Use [follow-up measurements](../development/TERN_BINARY_RENDERING_ANALYSIS.md#10-follow-up-measurements-and-ratification) for Release provenance, startup, idle counters, native presents, viewport workloads and media decode results. The [technical companion](../architecture/RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) owns detailed contracts, budgets, status splits and R0–R10 gates.

- **RECOMMENDED:** A for conventional surfaces, B for bounded artifact/graph requirements under the above constraints.
- **ALTERNATIVE:** A alone where it meets requirements; C (selected pin update) for a concrete verified fix/capability; D (selected independent renderer) only after GPUI-specific limitations and complete interop costs are demonstrated.
- **REJECTED now:** E (full-stack replacement), an independent fork, an assumed zero-copy path, unrestricted whole-image uploads, generic render/media abstractions and a benchmark ranking based on sparse-frame gaps or unequal features.

This is not a relative-throughput ranking of unbuilt stacks. No new renderer dependency is selected. Reopen this ADR only for a measured requirement failure, supported capability gap, security/stability fix or a complete Windows A/B that includes text/IME/AX/DPI, image/video, resource lifetime and present behavior.

## Consequences and rollback

Implementation proceeds through separately authorized R0–R10 phases. Each phase owns its observable regression and rollback gate; unavailable product features are not fake baseline results. The first priorities are reliable timing/idle attribution, stable localized state and virtualization, then bounded image/compare and graph presentation. Accessibility/DPI/IME validation starts with typography, not after media work.

Rollback removes a faulty optimization/effect or exposes a bounded supported preview/unavailable capability without weakening canonical truth, replaying unsafe commands, eagerly mounting an unbounded collection or silently allocating a full oversized image. Exact palette/font choices, calibrated 60/120-Hz SLOs, ICC/HDR, video interop and device-loss recovery are not certified by this ADR.
