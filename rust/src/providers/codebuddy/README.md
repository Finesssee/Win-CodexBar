# CodeBuddy (soft-fork)

Tencent **CodeBuddy CN** credit usage for Win-CodexBar.

## What it shows

- Primary meter: **Credits** used % across all returned resource packages
- Description: compact `remaining / total left` (fits the tray metric row)
- Reset time: earliest package expire time when present

## Setup

1. Settings → Providers → enable **CodeBuddy**
2. Prefer **manual Cookie** (Chrome 127+ App-Bound Encryption often blocks auto-import):

   - Open <https://www.codebuddy.cn/profile/plans-usage>
   - DevTools → Network → `get-user-resource` → Copy as cURL
   - Paste the `Cookie:` value into CodeBuddy settings / token account

3. Or put the cookie on one line in:

   ```
   %USERPROFILE%\.codebuddy\cb_cookie.txt
   ```

   (compatible with `D:\workspace\codebuddy-statusline`)

Auth priority: manual Cookie → `cb_cookie.txt` → browser cookies for
`codebuddy.cn`. Cookies are never logged; only a truncated SHA-256 fingerprint
of the cookie reaches the local cache (see below).

## API

```http
POST https://www.codebuddy.cn/billing/meter/get-user-resource
Origin: https://www.codebuddy.cn
Referer: https://www.codebuddy.cn/profile/plans-usage
User-Agent: Chrome/*  (must NOT contain Edg/)
Cookie: …
Content-Type: application/json

{
  "PageNumber": 1,
  "PageSize": 200,
  "ProductCode": "p_tcaca",
  "Status": [0, 3],
  "OnlyValidPeriod": true,
  "PackageCodes": [ "TCACA_code_007_…", … ]
}
```

Sums `CapacitySize* / CapacityUsed* / CapacityRemain*` over `data.Response.Data.Accounts`.

### EdgeOne 401

Tencent EdgeOne rejects Microsoft Edge user-agents on this path. The provider always sends a Chrome UA without `Edg/`.

Expired cookies also surface as 401 — refresh `cb_cookie.txt` / manual cookie.

### Empty Accounts

If `Accounts` is `[]`, your package codes differ. Copy `PackageCodes` from the browser request body and set:

```powershell
$env:CB_PACKAGE_CODES = '["TCACA_code_007_...","TCACA_code_029_..."]'
```

Entries are validated: strings only, trimmed, ≤ 128 chars, no control
characters, at most 64 codes.

### Endpoint override (advanced)

`CB_API_URL` may point the provider at another endpoint (useful for staging or
local mocks). It is validated: must be a well-formed `https:` URL; `http:` is
accepted only for loopback hosts (`localhost`, `127.0.0.1`, `::1`). Invalid
values are ignored with a warning and the default CN endpoint is used.

## Local cache fallback

Source mode **Auto** (transient web failures only) / **CLI** can read:

```
%USERPROFILE%\.codebuddy\cb_credits.json
```

also produced by `codebuddy-statusline`’s `parse_credits.js` pipeline.

Semantics:

- Written only after a **successful** web fetch, from typed numeric totals —
  never re-parsed out of the tray's display label.
- Expired/invalid cookies (HTTP 401/403 or an auth-flavoured API response)
  propagate as an auth error; they are **never** masked by the cache.
- The cache may carry `accountHash` (truncated SHA-256 of the cookie that
  wrote it); a cache belonging to a different account is rejected. Caches
  without a fingerprint (external helpers) are accepted.
- Schema: `{ total, used, remaining, source, updatedAt, resetsAt?, accountHash? }`
  — values are exact `f64` numbers, `resetsAt` RFC 3339.
- The API response is capped at 8 MiB and the cache file at 1 MiB.

## CLI

```text
codexbar-cli usage -p codebuddy --verbose
```

## Not covered yet

- International `codebuddy.ai` (different host / product codes)
- OAuth device flow (cookie session only)
