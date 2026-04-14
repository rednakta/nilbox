# OAuth Provider Scripts

This directory contains [Rhai](https://rhai.rs/) scripts that define OAuth provider behavior for the nilbox proxy.

Each script represents one OAuth provider. The nilbox proxy loads these scripts at runtime to intercept and handle OAuth flows initiated from inside the VM — injecting real credentials and forwarding token exchanges to the actual provider endpoints.

---

## How It Works

```
VM App  →  nilbox Proxy  →  OAuth Provider (e.g. Google, OpenAI)
               ↑
        Loads .rhai script
        to determine how to
        intercept & rewrite
        the OAuth flow
```

1. The VM app initiates an OAuth flow using placeholder credentials.
2. The nilbox proxy intercepts the request, detects the provider via `auth_domains`.
3. The script rewrites the request — substituting real credentials and redirecting token calls.
4. The proxy forwards the token exchange to the actual provider and returns the result.

---

## Required Functions

Every `.rhai` script **must** implement the following functions:

### `provider_info() -> Map`

Returns provider metadata.

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | Unique provider identifier (e.g. `"google"`) |
| `token_path` | String | Key used to look up credentials (env var name or `oauth:<name>`) |
| `placeholder_prefix` | String | Prefix for placeholder env vars injected into the VM (e.g. `"NILBOX_OAUTH_GOOGLE"`) |
| `auth_domains` | Array | Domains that trigger this provider's script (e.g. `["accounts.google.com"]`) |
| `token_path_pattern` | String | URL path pattern that identifies token exchange requests |
| `flow_type` | String | *(optional)* OAuth flow type: `"pkce"` or `"standard"` |
| `token_endpoint_domains` | Array | *(optional)* Domains that host the token endpoint |
| `cross_domains` | Array | *(optional)* Additional domains associated with this provider |

```rhai
fn provider_info() {
    #{
        name: "myprovider",
        token_path: "OAUTH_MYPROVIDER_FILE",
        placeholder_prefix: "NILBOX_OAUTH_MYPROVIDER",
        auth_domains: ["auth.myprovider.com"],
        token_path_pattern: "/oauth/token",
    }
}
```

---

### `placeholder_extraction_instructions() -> Map`

Defines how to extract credential values from the secret file.

The keys are short labels (`"ID"`, `"SECRET"`, etc.) combined with `placeholder_prefix` to form the full placeholder name. The values are JSON paths into the credential file.

```rhai
fn placeholder_extraction_instructions() {
    #{
        "ID":     "installed.client_id",
        "SECRET": "installed.client_secret",
    }
}
```

Return `#{}` if no extraction is needed (e.g. for PKCE flows where credentials are embedded in the app).

---

### `make_dummy_secret(prefix: String) -> String`

Returns a placeholder credential string that is injected into the VM environment. The VM app uses this dummy secret to start the OAuth flow; the proxy later substitutes the real values.

`prefix` is the `placeholder_prefix` value from `provider_info()`.

```rhai
fn make_dummy_secret(prefix) {
    let id     = prefix + "_ID";
    let secret = prefix + "_SECRET";
    `{"client_id":"` + id + `","client_secret":"` + secret + `"}`
}
```

Return `""` if no dummy secret is needed.

---

### `rewrite_auth_url(url: String, placeholders: Map) -> String`

Rewrites the authorization URL before redirecting the VM app to the provider. Use this to replace placeholder values in the URL with real credentials.

`placeholders` contains the extracted values keyed by the labels defined in `placeholder_extraction_instructions()`.

```rhai
fn rewrite_auth_url(url, placeholders) {
    url.replace("NILBOX_OAUTH_MYPROVIDER_ID", placeholders["ID"])
}
```

Return `url` unchanged if no rewriting is required.

---

### `build_token_request_instructions(body_params: Map) -> Map`

Returns instructions for forwarding the token exchange request to the real provider.

| Field | Type | Description |
|-------|------|-------------|
| `field_substitutions` | Map | Body fields to replace (`placeholder → real value`) |
| `target_url` | String | Token endpoint URL (empty = pass through as-is) |
| `allowed_redirect_hosts` | Array | Hosts allowed in `redirect_uri` (e.g. `["localhost"]`) |

```rhai
fn build_token_request_instructions(body_params) {
    #{
        field_substitutions: #{},
        target_url: "https://auth.myprovider.com/oauth/token",
        allowed_redirect_hosts: ["localhost", "127.0.0.1"],
    }
}
```

---

## Optional Functions

### `token_response_fields() -> Map`

Maps standard field names to provider-specific JSON keys in the token response. Implement this if the provider uses non-standard field names.

```rhai
fn token_response_fields() {
    #{
        access_token_field:  "access_token",
        refresh_token_field: "refresh_token",
        expires_in_field:    "expires_in",
        token_type_field:    "token_type",
        response_format:     "json",
    }
}
```

### `is_token_exchange_request(body_params: Map) -> bool`

Returns `true` if the intercepted request should be treated as a token exchange. Implement this to override the default detection logic.

```rhai
fn is_token_exchange_request(body_params) {
    let gt = body_params.get("grant_type");
    gt == "authorization_code" || gt == "refresh_token"
}
```

---

## Installing a Custom Script

### 1. Enable Developer Mode

Open **Settings** and turn on **Developer Mode**.  
This unlocks the custom script option in the Mappings screen.

### 2. Add the Script

Go to **Mappings → OAuth → + Custom**.  
Select your `.rhai` file and confirm.  
The proxy loads the script immediately — no restart required.

---

## Adding a New Provider (Script Authoring)

1. Create `<provider_name>.rhai` in this directory.
2. Implement all required functions above.
3. Add optional functions as needed.
4. Install via the steps above.

---

## Existing Providers

| File | Provider | Flow |
|------|----------|------|
| `google.rhai` | Google OAuth 2.0 | Standard (client_secret JSON file) |
| `openai.rhai` | OpenAI / ChatGPT | PKCE |
