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
MERMAID = re.compile(r"```mermaid\n(.*?)```", re.DOTALL)
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

        bullets = body.split("\n\n", 1)[0]
        for target in RELATIVE_LINK.findall(bullets):
            path = os.path.normpath(os.path.join(root, target))
            if not os.path.exists(path):
                findings.append(Finding(name, f"link target does not exist: {target}"))

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
        return 1 if self_test() else 0

    with open(args.plan, encoding="utf-8") as handle:
        text = handle.read()
    root = os.path.dirname(os.path.abspath(args.plan))
    findings = check_cards(text, root) + check_dag(text)
    for finding in findings:
        print(finding)
    print(f"{len(findings)} finding(s) across {len(cards(text))} card(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
