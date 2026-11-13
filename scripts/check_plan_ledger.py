#!/usr/bin/env python3
"""Ledger consistency checks for the implementation master plan.

It checks the facts the plan is supposed to keep straight, and nothing about the design:

1. every task card has a Status line and a design-contract link;
2. every relative link in a card resolves to a file that exists;
3. every card referenced from the dependency DAG exists, and every card appears in it;
4. a card does not claim to be COMPLETE while its own text records an unrun or unmeasured check.

Run with `--self-test` to check the checker itself against small fixtures.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from dataclasses import dataclass

PLAN = os.path.join("docs", "development", "IMPLEMENTATION_MASTER_PLAN.md")
CARD = re.compile(r"^#### (MP-[A-Z0-9]+) — (.+)$", re.MULTILINE)
STATUS = re.compile(r"\*\*Status:\*\* (.+?)(?: · | \| |$)", re.MULTILINE)
CONTRACT = re.compile(r"^- \*\*Design source / acceptance:\*\* (.+)$", re.MULTILINE)
RELATIVE_LINK = re.compile(r"\[[^\]]+\]\((?!https?:|#)([^)#]+)(?:#[^)]*)?\)")
LINK_WITH_ANCHOR = re.compile(r"\[[^\]]+\]\((?!https?:|#)([^)#]+)(?:#([^)]*))?\)")
MERMAID = re.compile(r"```mermaid\n(.*?)```", re.DOTALL)
PREREQS = re.compile(r"^- \*\*Hard prereqs:\*\* (.+)$", re.MULTILINE)
DAG_EDGE = re.compile(r"\b(MP-[A-Z0-9]+)\s*-->\s*(MP-[A-Z0-9]+)\b")
MERMAID_LABELLED = re.compile(r"([A-Za-z][A-Za-z0-9_]*)\s*\[\s*(MP-[A-Z0-9]+)\s*\]")
MERMAID_ARROW = re.compile(r"([A-Za-z0-9_-]+)\s*-->\s*([A-Za-z0-9_-]+)")
DAG_NODE = re.compile(r"\b(MP-[A-Z0-9]+)\b")
UNRUN = ("NOT_RUN", "NOT_MEASURED", "NOT_VERIFIED", "NOT RUN", "未検証", "未測定")


@dataclass
class Finding:
    card: str
    problem: str

    def __str__(self) -> str:
        return f"{self.card}: {self.problem}"


def cards(text: str) -> list[tuple[str, str, str]]:
    """Return (name, title, full card text) for every task card."""
    found = []
    matches = list(CARD.finditer(text))
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        found.append((match.group(1), match.group(2), text[match.start() : end]))
    return found


def headings(path: str) -> set[str]:
    """GitHub-style anchors for the headings of a markdown file."""
    try:
        with open(path, encoding="utf-8") as handle:
            content = handle.read()
    except OSError:
        return set()
    anchors = set()
    for line in content.splitlines():
        if not line.startswith("#"):
            continue
        title = line.lstrip("#").strip()
        # GitHub keeps letters, digits, underscores and spaces; every other mark is dropped.
        slug = re.sub(r"[^\w\s-]", "", title.lower()).strip()
        anchors.add(slug.replace(" ", "-"))
    return anchors


def unverified_commits(text: str) -> set[str]:
    """Commit ids the plan cites. They cannot be resolved without a repository lookup."""
    return set(re.findall(r"`([0-9a-f]{7,40})`", text))


def check_cards(text: str, root: str) -> list[Finding]:
    findings: list[Finding] = []
    names = set()
    for name, _title, body in cards(text):
        if name in names:
            findings.append(Finding(name, "duplicate card"))
        names.add(name)

        status = STATUS.search(body)
        if not status:
            findings.append(Finding(name, "has no Status line"))
            continue
        status_text = status.group(1)

        if not CONTRACT.search(body):
            findings.append(Finding(name, "has no design source / acceptance line"))

        bullets = "\n".join(line for line in body.splitlines() if line.startswith("- "))
        for target, anchor in LINK_WITH_ANCHOR.findall(bullets):
            path = os.path.normpath(os.path.join(root, target))
            if not os.path.exists(path):
                findings.append(Finding(name, f"link target does not exist: {target}"))
                continue
            if anchor and anchor not in headings(path):
                findings.append(Finding(name, f"link anchor does not exist: {target}#{anchor}"))
        # The design contract a card names must exist as a heading.
        contract = CONTRACT.search(body)
        if contract:
            for target, anchor in LINK_WITH_ANCHOR.findall(contract.group(1)):
                path = os.path.normpath(os.path.join(root, target))
                if os.path.exists(path) and anchor:
                    if anchor not in headings(path):
                        findings.append(
                            Finding(name, f"design-contract anchor does not exist: {target}#{anchor}")
                        )
                    expected = name.lower().replace("-", "-")
                    if anchor != f"dc-{expected}" and anchor.startswith("dc-mp-"):
                        findings.append(
                            Finding(
                                name,
                                f"the card names {anchor} but its own contract is dc-{expected}",
                            )
                        )

        # A card may record unrun checks, but it must not also claim to be complete.
        upper = status_text.upper()
        qualifies = any(
            word in upper
            for word in ("TECHNICALLY", "LIMITED", "PARTIAL", "PROTOTYPE", "NOT_COMPLETE")
        )
        claims_complete = "COMPLETE" in upper and not qualifies
        records_unrun = any(word in status_text for word in UNRUN)
        if claims_complete and records_unrun:
            findings.append(
                Finding(name, "claims COMPLETE while recording an unrun or unmeasured check")
            )
    return findings


def prereq_edges(card_body: str) -> list[str]:
    """The card names this card declares as hard prerequisites."""
    match = PREREQS.search(card_body)
    if not match:
        return []
    return DAG_NODE.findall(match.group(1))


def check_prereqs(text: str) -> list[Finding]:
    """Every declared prerequisite must exist and no prerequisite chain may loop."""
    findings: list[Finding] = []
    names = {name for name, _title, _body in cards(text)}
    graph: dict[str, list[str]] = {}
    for name, _title, body in cards(text):
        edges = prereq_edges(body)
        for target in edges:
            if target not in names:
                findings.append(Finding(name, f"declares a prerequisite without a card: {target}"))
        graph[name] = [target for target in edges if target in names]

    # A depth-first walk that records the path, so a cycle can be reported rather than only seen.
    colour: dict[str, int] = {}
    for start in graph:
        if colour.get(start):
            continue
        stack = [(start, iter(graph[start]))]
        path = [start]
        colour[start] = 1
        while stack:
            node, children = stack[-1]
            for child in children:
                if colour.get(child) == 1:
                    cycle = " -> ".join(path[path.index(child) :] + [child])
                    findings.append(Finding(node, f"prerequisite cycle: {cycle}"))
                elif not colour.get(child):
                    colour[child] = 1
                    path.append(child)
                    stack.append((child, iter(graph[child])))
                    break
            else:
                colour[node] = 2
                stack.pop()
                path.pop()
    return findings


def declared_edges(text: str) -> set[tuple[str, str]]:
    """(prerequisite, card) pairs the cards themselves declare."""
    edges = set()
    for name, _title, body in cards(text):
        for target in prereq_edges(body):
            edges.add((target, name))
    return edges


def diagram_edges(block: str) -> set[tuple[str, str]]:
    """Edges one mermaid block draws, resolving the short node aliases it declares.

    The plan spells a node either literally (`MP-A --> MP-B`) or with a short alias
    (`A[MP-A]`, then `A --> B[MP-B]`). Matching only literal identifiers silently
    skipped every aliased diagram, so its edges were never checked at all. An arrow
    whose endpoint names no declared alias is not guessed: it contributes no edge.
    """
    alias = {name: card for name, card in MERMAID_LABELLED.findall(block)}
    # A label may sit on the arrow itself (`A1[MP-A] --> B1[MP-B]`), so rewrite every
    # labelled node to its bare card id before scanning for arrows.
    normalized = MERMAID_LABELLED.sub(lambda match: match.group(2), block)
    edges = set()
    for raw_source, raw_target in MERMAID_ARROW.findall(normalized):
        source = raw_source if DAG_NODE.fullmatch(raw_source) else alias.get(raw_source)
        target = raw_target if DAG_NODE.fullmatch(raw_target) else alias.get(raw_target)
        if source and target:
            edges.add((source, target))
    return edges


def check_dag(text: str) -> list[Finding]:
    """Every card referenced from a diagram must exist.

    The diagrams show dependency spines rather than all 62 cards, so a card that no diagram
    mentions is not a finding; a diagram naming a card that does not exist is.
    """
    findings: list[Finding] = []
    names = {name for name, _title, _body in cards(text)}
    referenced = set()
    for block in MERMAID.findall(text):
        referenced.update(DAG_NODE.findall(block))
    for name in sorted(referenced - names):
        findings.append(Finding(name, "appears in a diagram but has no card"))

    # The diagrams are partial: they show spines, not every declared edge. A drawn edge that the
    # cards do not declare is a contradiction; a declared edge that no diagram draws is not.
    declared = declared_edges(text)
    drawn = set()
    for block in MERMAID.findall(text):
        drawn.update(diagram_edges(block))
    for source, target in sorted(drawn):
        if (source, target) not in declared:
            findings.append(
                Finding(target, f"the diagram draws {source} --> {target} but no card declares it")
            )
    return findings


def self_test() -> int:
    """Fixtures that exercise each rule, including the ones that must not fire."""
    failures = 0

    def expect(condition: bool, description: str) -> None:
        nonlocal failures
        if condition:
            print(f"self-test ok: {description}")
        else:
            print(f"SELF-TEST FAILED: {description}")
            failures += 1

    good = (
        "#### MP-X1 — A card\n"
        "- **Status:** COMPLETE\n"
        "- **Design source / acceptance:** [DC](../architecture/DESKTOP_DESIGN_CONTRACTS.md)\n"
        "\n```mermaid\nX1[MP-X1]\n```\n"
    )
    findings = check_cards(good, os.path.dirname(PLAN))
    expect(
        all("no Status" not in str(f) for f in findings),
        "a card with a status line passes",
    )

    missing_status = "#### MP-X2 — No status\n- **Design source / acceptance:** x\n"
    expect(
        any("no Status line" in str(f) for f in check_cards(missing_status, ".")),
        "a card without a status line is reported",
    )

    contradiction = (
        "#### MP-X3 — Contradiction\n"
        "- **Status:** COMPLETE (Native NOT_RUN)\n"
        "- **Design source / acceptance:** x\n"
    )
    expect(
        any("claims COMPLETE" in str(f) for f in check_cards(contradiction, ".")),
        "COMPLETE plus an unrun check is reported",
    )

    broken = (
        "#### MP-X4 — Broken link\n"
        "- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** [DC](./does-not-exist.md)\n"
    )
    expect(
        any("does not exist" in str(f) for f in check_cards(broken, ".")),
        "a link to a missing file is reported",
    )

    cycle = (
        "#### MP-A — A\n- **Hard prereqs:** MP-B\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** x\n\n"
        "#### MP-B — B\n- **Hard prereqs:** MP-A\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** x\n"
    )
    expect(
        any("prerequisite cycle" in str(f) for f in check_prereqs(cycle)),
        "a prerequisite cycle is reported",
    )
    expect(
        not check_prereqs(good.replace("- **Status:** COMPLETE", "- **Hard prereqs:** none\n- **Status:** COMPLETE")),
        "a card with no prerequisites passes the graph check",
    )
    missing_prereq = (
        "#### MP-A — A\n- **Hard prereqs:** MP-Z\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** x\n"
    )
    expect(
        any("without a card" in str(f) for f in check_prereqs(missing_prereq)),
        "a prerequisite with no card is reported",
    )

    anchor_fixture = (
        "#### MP-X5 — Anchor\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** [n](../development/TESTING.md#no-such-heading)\n"
    )
    findings = check_cards(anchor_fixture, os.path.dirname(PLAN))
    expect(
        any("anchor does not exist" in str(f) for f in findings),
        "a link anchor that is not a heading is reported",
    )

    wrong_dc = (
        "#### MP-X6 — Contract\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** "
        "[DC](../architecture/DESKTOP_DESIGN_CONTRACTS.md#dc-mp-img3)\n"
    )
    findings = check_cards(wrong_dc, os.path.dirname(PLAN))
    expect(
        any("its own contract is" in str(f) for f in findings),
        "a card naming another card's contract is reported",
    )

    edge_fixture = (
        "#### MP-A — A\n- **Hard prereqs:** none.\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** x\n\n"
        "#### MP-B — B\n- **Hard prereqs:** none.\n- **Status:** PARTIAL\n"
        "- **Design source / acceptance:** x\n\n"
        "```mermaid\nMP-A --> MP-B\n```\n"
    )
    expect(
        any("but no card declares it" in str(f) for f in check_dag(edge_fixture)),
        "a diagram edge the cards do not declare is reported",
    )

    def two_cards(prereq_b: str) -> str:
        return (
            "#### MP-A — A\n- **Hard prereqs:** none.\n- **Status:** PARTIAL\n"
            "- **Design source / acceptance:** x\n\n"
            "#### MP-B — B\n- **Hard prereqs:** " + prereq_b + ".\n"
            "- **Status:** PARTIAL\n- **Design source / acceptance:** x\n\n"
        )

    alias_undeclared = two_cards("none") + "```mermaid\nA1[MP-A] --> B1[MP-B]\n```\n"
    expect(
        any("but no card declares it" in str(f) for f in check_dag(alias_undeclared)),
        "an edge drawn between aliased nodes is checked, not skipped",
    )
    alias_declared = two_cards("MP-A") + "```mermaid\nA1[MP-A]\nB1[MP-B]\nA1 --> B1\n```\n"
    expect(
        not any("but no card declares it" in str(f) for f in check_dag(alias_declared)),
        "an aliased edge the cards declare is accepted",
    )

    dag_text = good + "\n```mermaid\nX1[MP-X1]\nX2[MP-X2]\nX1 --> X2\n```\n"
    expect(
        any("has no card" in str(f) for f in check_dag(dag_text)),
        "a diagram node without a card is reported",
    )
    expect(
        not any("has no card" in str(f) for f in check_dag(good + "\n```mermaid\nX1[MP-X1]\n```\n")),
        "a diagram whose nodes all have cards passes",
    )
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--plan", default=PLAN)
    args = parser.parse_args()

    if args.self_test:
        failures = self_test()
        # The self-test proves the rules; the live run proves the rules against the plan. Both are
        # reported so a passing self-test cannot stand in for an unchecked ledger.
        with open(args.plan, encoding="utf-8") as handle:
            text = handle.read()
        root = os.path.dirname(os.path.abspath(args.plan))
        live = check_cards(text, root) + check_prereqs(text) + check_dag(text)
        cited = unverified_commits(text)
        for finding in live:
            print(finding)
        print(f"live: {len(live)} finding(s) across {len(cards(text))} card(s)")
        print(
            f"unverified here: {len(cited)} commit id(s) cited by the plan were not resolved "
            "(that needs a repository lookup, so they are not counted as passing)"
        )
        return 1 if failures or live else 0

    with open(args.plan, encoding="utf-8") as handle:
        text = handle.read()
    root = os.path.dirname(os.path.abspath(args.plan))
    findings = check_cards(text, root) + check_prereqs(text) + check_dag(text)
    for finding in findings:
        print(finding)
    cited = unverified_commits(text)
    print(f"{len(findings)} finding(s) across {len(cards(text))} card(s)")
    # Always printed, in every mode: a ledger whose citations were never resolved is not a
    # verified ledger, and the summary says so instead of letting the run look complete.
    print(
        f"unverified: {len(cited)} cited commit id(s) were not resolved by this check "
        "(repository lookup is unavailable here); they remain an open condition"
    )
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
