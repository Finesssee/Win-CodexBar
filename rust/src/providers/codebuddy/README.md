# CodeBuddy (soft-fork)

Tencent **CodeBuddy CN** credit usage for Win-CodexBar.

## What it shows

- Primary meter: **Credits** used % across all returned resource packages
- Description: `remaining/total` plus package count
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

## Local cache fallback

Source mode **Auto** / **CLI** can read:

```
%USERPROFILE%\.codebuddy\cb_credits.json
```

produced by `codebuddy-statusline`’s `parse_credits.js` pipeline.

## CLI

```text
codexbar-cli usage -p codebuddy --verbose
```

## Not covered yet

- International `codebuddy.ai` (different host / product codes)
- OAuth device flow (cookie session only)
