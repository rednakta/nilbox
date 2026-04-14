//! HTTP request parser

use anyhow::{Result, anyhow};
use std::collections::HashMap;

#[derive(Debug)]
pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body_offset: usize,
}

pub fn parse_request_headers(data: &[u8]) -> Result<ParsedRequest> {
    let mut headers = [httparse::Header { name: "", value: &[] }; 64];
    let mut req = httparse::Request::new(&mut headers);

    match req.parse(data) {
        Ok(httparse::Status::Complete(offset)) => {
            let method = req.method.ok_or(anyhow!("Missing method"))?.to_string();
            let path = req.path.ok_or(anyhow!("Missing path"))?.to_string();
            let mut headers_map = HashMap::new();
            for h in req.headers {
                if !h.name.is_empty() {
                    headers_map.insert(
                        h.name.to_lowercase(),
                        String::from_utf8_lossy(h.value).to_string(),
                    );
                }
            }
            Ok(ParsedRequest { method, path, headers: headers_map, body_offset: offset })
        }
        Ok(httparse::Status::Partial) => Err(anyhow!("Incomplete request headers")),
        Err(e) => Err(anyhow!("HTTP parse error: {}", e)),
    }
}
