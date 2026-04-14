// Google OAuth provider script

fn provider_info() {
    #{
        name: "google",
        token_path: "OAUTH_GOOGLE_FILE",
        placeholder_prefix: "NILBOX_OAUTH_GOOGLE",
        auth_domains: ["accounts.google.com"],
        token_path_pattern: "/oauth2/token",
    }
}

fn placeholder_extraction_instructions() {
    #{
        "ID": "installed.client_id",
        "SECRET": "installed.client_secret",
    }
}

fn make_dummy_secret(prefix) {
    let cid = prefix + "_ID";
    let csec = prefix + "_SECRET";
    "{\n  \"installed\": {\n    \"client_id\": \"" + cid
    + "\",\n    \"client_secret\": \"" + csec
    + "\",\n    \"auth_uri\": \"https://accounts.google.com/o/oauth2/auth\""
    + ",\n    \"token_uri\": \"http://127.0.0.1:8088/oauth2/token\""
    + ",\n    \"redirect_uris\": [\"http://localhost\"]\n  }\n}"
}

fn rewrite_auth_url(url, placeholders) { url }

fn build_token_request_instructions(body_params) {
    #{
        field_substitutions: #{
            "client_id": "ID",
            "client_secret": "SECRET",
        },
        target_url: "https://oauth2.googleapis.com/token",
        allowed_redirect_hosts: ["localhost", "127.0.0.1", "::1"],
    }
}