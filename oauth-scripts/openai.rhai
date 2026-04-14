fn provider_info() {
      #{
          name: "openai",
          token_path: "oauth:openai",
          placeholder_prefix: "",
          auth_domains: ["auth.openai.com"],
          token_path_pattern: "/oauth/token",
          flow_type: "pkce",
          token_endpoint_domains: ["auth.openai.com"],
          cross_domains: ["chatgpt.com"],
      }
  }
  fn placeholder_extraction_instructions() { #{} }
  fn make_dummy_secret(prefix) { "" }
  fn rewrite_auth_url(url, placeholders) { url }
  fn build_token_request_instructions(body_params) {
      #{ field_substitutions: #{}, target_url: "https://auth.openai.com/oauth/token", allowed_redirect_hosts: ["localhost", "127.0.0.1"] }
  }
  fn token_response_fields() {
      #{
          access_token_field: "access_token",
          refresh_token_field: "refresh_token",
          expires_in_field: "expires_in",
          token_type_field: "token_type",
          response_format: "json",
      }
  }
  fn is_token_exchange_request(body_params) {
      let gt = body_params.get("grant_type");
      gt == "authorization_code" || gt == "refresh_token"
  }