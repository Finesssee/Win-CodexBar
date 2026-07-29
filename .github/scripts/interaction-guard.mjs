const accountAgeDays = 30;
const windowDays = 7;
const limits = {
  issue: { count: 10, label: "issues" },
  pull_request: { count: 4, label: "pull requests" },
};
// Must match the fromJSON list in the guard job's `if:` in
// .github/workflows/interaction-guard.yml.
const trustedAssociations = new Set(["OWNER", "MEMBER", "COLLABORATOR"]);

export function isTrustedAuthor(authorAssociation) {
  return trustedAssociations.has(authorAssociation);
}

export function closePayload(kind) {
  // state_reason is only valid for issues; PATCHing it on a pull request
  // fails with 403 "Insufficient permissions to update the state_reason".
  return kind === "pull_request" ? { state: "closed" } : { state: "closed", state_reason: "not_planned" };
}

export function evaluateInteraction({ kind, userCreatedAt, now, recentCount }) {
  const created = Date.parse(userCreatedAt);
  const current = Date.parse(now);
  if (!Number.isFinite(created) || !Number.isFinite(current)) {
    return { allowed: false, reason: "Unable to verify account age." };
  }

  const ageDays = Math.floor((current - created) / 86_400_000);
  if (ageDays < accountAgeDays) {
    return {
      allowed: false,
      reason: `Your GitHub account must be at least ${accountAgeDays} days old to open issues or pull requests in this repository.`,
    };
  }

  const limit = limits[kind];
  if (!limit) {
    return { allowed: true };
  }

  if (recentCount > limit.count) {
    return {
      allowed: false,
      reason: `This repository allows each user to open at most ${limit.count} ${limit.label} per ${windowDays} days.`,
    };
  }

  return { allowed: true };
}

function eventTarget(payload) {
  if (payload.pull_request) {
    return {
      kind: "pull_request",
      number: payload.pull_request.number,
      author: payload.pull_request.user.login,
      authorAssociation: payload.pull_request.author_association,
    };
  }
  if (payload.issue && !payload.issue.pull_request) {
    return {
      kind: "issue",
      number: payload.issue.number,
      author: payload.issue.user.login,
      authorAssociation: payload.issue.author_association,
    };
  }
  return null;
}

function sinceDate(now) {
  const since = new Date(now);
  since.setUTCDate(since.getUTCDate() - windowDays);
  return since.toISOString().slice(0, 10);
}

async function github(path, options = {}) {
  const response = await fetch(`https://api.github.com${path}`, {
    ...options,
    headers: {
      accept: "application/vnd.github+json",
      authorization: `Bearer ${process.env.GITHUB_TOKEN}`,
      "x-github-api-version": "2022-11-28",
      ...options.headers,
    },
  });
  if (!response.ok) {
    throw new Error(`${options.method ?? "GET"} ${path} failed: ${response.status} ${await response.text()}`);
  }
  return response.status === 204 ? null : response.json();
}

async function main() {
  const fs = await import("node:fs/promises");
  const payload = JSON.parse(await fs.readFile(process.env.GITHUB_EVENT_PATH, "utf8"));
  const target = eventTarget(payload);
  if (!target) return;
  if (isTrustedAuthor(target.authorAssociation)) return;

  const repo = process.env.GITHUB_REPOSITORY;
  const now = new Date().toISOString();
  const user = await github(`/users/${target.author}`);
  const type = target.kind === "pull_request" ? "pr" : "issue";
  const query = encodeURIComponent(`repo:${repo} author:${target.author} type:${type} created:>=${sinceDate(now)}`);
  const recent = await github(`/search/issues?q=${query}&per_page=1`);
  const result = evaluateInteraction({
    kind: target.kind,
    userCreatedAt: user.created_at,
    now,
    recentCount: recent.total_count,
  });

  if (result.allowed) return;

  const body = `${result.reason}\n\nClosing this automatically.`;
  await github(`/repos/${repo}/issues/${target.number}/comments`, {
    method: "POST",
    body: JSON.stringify({ body }),
  });
  await github(`/repos/${repo}/issues/${target.number}`, {
    method: "PATCH",
    body: JSON.stringify(closePayload(target.kind)),
  });
}

if (import.meta.url === `file://${process.argv[1].replaceAll("\\", "/")}`) {
  await main();
}
