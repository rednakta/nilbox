// GitHub OAuth provider script
// Supports web flow (gh auth login --web)
// Note: Device flow (default gh auth login) is not fully supported because the
// /login/device/code endpoint is not intercepted by the proxy's token_path_pattern.
// Use --web flag for full OAuth support.

fn provider_info() {
    #{
        name: "github",
        token_path: "OAUTH_GITHUB_FILE",
        placeholder_prefix: "NILBOX_OAUTH_GITHUB",
        auth_domains: ["github.com"],
        token_path_pattern: "/login/oauth/access_token",
    }
}

fn placeholder_extraction_instructions() {
    #{
        "ID": "client_id",
        "SECRET": "client_secret",
    }
}

fn make_dummy_secret(prefix) {
    let id = prefix + "_ID";
    let secret = prefix + "_SECRET";
    `{"client_id":"` + id + `","client_secret":"` + secret + `"}`
}

fn rewrite_auth_url(url, placeholders) {
    url.replace("NILBOX_OAUTH_GITHUB_ID", placeholders["ID"])
}

fn build_token_request_instructions(body_params) {
    #{
        field_substitutions: #{
            "client_id": "ID",
            "client_secret": "SECRET",
        },
        target_url: "https://github.com/login/oauth/access_token",
        allowed_redirect_hosts: ["localhost", "127.0.0.1"],
    }
}

fn is_token_exchange_request(body_params) {
    // Web flow: code parameter present (no grant_type)
    // Device flow: grant_type = "urn:ietf:params:oauth:grant-type:device_code"
    // Refresh flow: grant_type = "refresh_token" (GitHub Apps only, not OAuth Apps)
    body_params.contains_key("code")
        || body_params.get("grant_type") == "urn:ietf:params:oauth:grant-type:device_code"
        || body_params.get("grant_type") == "refresh_token"
}

fn token_response_fields() {
    #{
        access_token_field: "access_token",
        refresh_token_field: "",           // GitHub OAuth Apps do not return refresh_token
        expires_in_field: "",              // GitHub OAuth Apps do not have expiring tokens
        token_type_field: "token_type",
        response_format: "form",           // GitHub returns application/x-www-form-urlencoded
    }
}
