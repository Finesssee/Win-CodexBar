import assert from "node:assert/strict";
import test from "node:test";

import { closePayload, evaluateInteraction, isTrustedAuthor } from "./interaction-guard.mjs";

const now = "2026-07-08T00:00:00.000Z";

test("blocks accounts younger than 30 days", () => {
  const result = evaluateInteraction({
    kind: "issue",
    author: "new-user",
    userCreatedAt: "2026-06-20T00:00:00.000Z",
    now,
    recentCount: 1,
  });

  assert.equal(result.allowed, false);
  assert.match(result.reason, /at least 30 days old/);
});

test("allows old accounts under the weekly issue limit", () => {
  const result = evaluateInteraction({
    kind: "issue",
    author: "steady-user",
    userCreatedAt: "2026-05-01T00:00:00.000Z",
    now,
    recentCount: 10,
  });

  assert.equal(result.allowed, true);
});

test("blocks the 11th issue in 7 days", () => {
  const result = evaluateInteraction({
    kind: "issue",
    author: "spammy-user",
    userCreatedAt: "2026-05-01T00:00:00.000Z",
    now,
    recentCount: 11,
  });

  assert.equal(result.allowed, false);
  assert.match(result.reason, /10 issues per 7 days/);
});

test("blocks the 5th pull request in 7 days", () => {
  const result = evaluateInteraction({
    kind: "pull_request",
    author: "spammy-user",
    userCreatedAt: "2026-05-01T00:00:00.000Z",
    now,
    recentCount: 5,
  });

  assert.equal(result.allowed, false);
  assert.match(result.reason, /4 pull requests per 7 days/);
});

test("trusts maintainer associations", () => {
  for (const authorAssociation of ["OWNER", "MEMBER", "COLLABORATOR"]) {
    assert.equal(isTrustedAuthor(authorAssociation), true);
  }
});

test("does not trust other associations", () => {
  assert.equal(isTrustedAuthor("CONTRIBUTOR"), false);
  assert.equal(isTrustedAuthor("NONE"), false);
  assert.equal(isTrustedAuthor(undefined), false);
});

test("close payload omits state_reason for pull requests", () => {
  assert.deepEqual(closePayload("pull_request"), { state: "closed" });
  assert.deepEqual(closePayload("issue"), { state: "closed", state_reason: "not_planned" });
});
