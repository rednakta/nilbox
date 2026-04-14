<p align="center">
  <img src="apps/nilbox/nilbox-title-logo.jpg" width="300" alt="nilbox Logo" style="vertical-align: middle; margin-right: 12px;">
</p>

<p align="center">
  <strong>신뢰할 수 없는 AI 에이전트를 실행하기 위한 데스크톱 샌드박스 — 실제 VM 격리와 제로 토큰 보안.</strong>
</p>

<p align="center">
  <a href="#빠른-시작">빠른 시작</a> ·
  <a href="#사용-사례-openclaw">사용 사례</a> ·
  <a href="#작동-방식">작동 방식</a> ·
  <a href="#기능">기능</a> ·
  <a href="#문서">문서</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/License-GPLv3-blue.svg" alt="License: GPL v3">
  <img src="https://img.shields.io/badge/version-0.1.8-green.svg" alt="Version">
  <img src="https://img.shields.io/badge/macOS-supported-success?logo=apple" alt="macOS">
  <img src="https://img.shields.io/badge/Linux-supported-success?logo=linux&logoColor=white" alt="Linux">
  <img src="https://img.shields.io/badge/Windows-supported-success?logo=windows&logoColor=white" alt="Windows">
</p>

---

## 왜 nilbox인가?

AI 에이전트는 셸 접근, 파일시스템 접근, 그리고 외부 API 호출이 필요합니다. 호스트 커널의 컨테이너에서 실행하는 것은 진정한 격리가 아닙니다 — 특히 그 에이전트가 실제 자격 증명을 처리할 때는 더욱 그렇습니다.

nilbox는 다른 접근 방식을 취합니다:

- **실제 VM 격리** — 워크로드가 컨테이너가 아닌 완전한 가상 머신에서 실행
- **제로 토큰 아키텍처** — API 키가 게스트에 절대 진입하지 않음; 호스트 프록시가 신뢰된 도메인에 한해 토큰을 실시간으로 교체
- **호스트 제어 네트워크** — 모든 아웃바운드 트래픽이 VSOCK을 통해 도메인 게이팅 프록시로 라우팅되며 속도 제한과 승인 프롬프트 포함

누군가에게 API 키를 주지 않을 것이라면, 그 키를 그들의 코드가 실행되는 곳에 두지 마세요.

---

## 빠른 시작

### 다운로드

[GitHub Releases](https://github.com/paiml/nilbox/releases)에서 플랫폼에 맞는 최신 릴리스를 받으세요.

### 소스에서 빌드

**전제 조건:** [Rust](https://rustup.rs/) 툴체인, [Node.js](https://nodejs.org/) 18+

```bash
git clone https://github.com/paiml/nilbox.git
cd nilbox

# 데스크톱 앱 실행
cd apps/nilbox && npm install && npm run tauri dev
```

전체 빌드 지침 및 릴리스 빌드는 [개발 가이드](docs/development.md)를 참조하세요.

---

## 사용 사례: OpenClaw

OpenClaw 같은 자율 AI 코딩 에이전트를 실행한다고 가정해봅시다. OpenAI, Anthropic, GitHub의 API 키와 코드 작성·실행을 위한 셸 접근이 필요합니다. 많은 신뢰가 필요한 작업입니다.

**nilbox 없이** (기존 Docker/호스트 방식):

```bash
# 컨테이너 내부 — 실제 키가 완전히 노출됨
$ echo $OPENAI_KEY
sk-proj-abc1234567890xyz...    # 실제 토큰, 탈취 가능
```

단 하나의 프롬프트 인젝션이나 악성 의존성이 이 키를 읽고, 외부로 유출하며, API 예산을 소진할 수 있습니다.

**nilbox 사용 시:**

```bash
# VM 내부 — 더미 값만 존재
$ echo $OPENAI_KEY
OPENAI_KEY                     # 그냥 문자열, 공격자에게 무용지물
```

**멀티 프로바이더 토큰 설정** — nilbox에서 각 프로바이더의 환경변수를 구성합니다. OpenClaw는 아래 예시와 같이 토큰이름만을 볼 수 있으며, nilbox 프록시가 신뢰된 도메인에 한해서만 실제 자격 증명으로 교체합니다:

```
# Claude (Anthropic)
ANTHROPIC_API_KEY=ANTHROPIC_API_KEY

# AWS Bedrock
AWS_ACCESS_KEY_ID=AWS_ACCESS_KEY_ID
AWS_SECRET_ACCESS_KEY=AWS_SECRET_ACCESS_KEY

# Gemini
GEMINI_API_KEY=GEMINI_API_KEY
```

에이전트가 `api.openai.com`에 정상적인 API 호출을 할 때, nilbox 프록시가 호스트에서 가로채어 `OPENAI_KEY`를 실제 토큰으로 교체한 후 전달합니다. 악성 페이로드가 `attacker.evil.com`으로 키를 보내려 하면, 프록시가 해당 도메인을 완전히 차단하거나 더미 문자열만 전달합니다 — **실제 토큰은 절대 호스트를 벗어나지 않습니다**.

**코드 변경 불필요.** OpenClaw 또는 다른 어떤 에이전트도 VM 내부에서 수정 없이 실행됩니다. 환경변수를 읽고 API 호출을 마치 베어메탈에서처럼 정확히 동일하게 수행합니다. 토큰 교체는 게스트 외부의 호스트 프록시 레이어에서 투명하게 이루어집니다. 에이전트, 의존성, 스크립트를 패치할 필요가 없습니다.

결과:
- 침해 후 키 교체 불필요 — 실제 토큰이 노출된 적 없음
- 예산 충격 없음 — 프로바이더별 지출 한도가 과다 사용을 차단
- 데이터 유출 없음 — VM은 승인한 도메인에만 접근 가능

공격 시나리오와 방어 레이어는 [제로 토큰 아키텍처](docs/zero-token-architecture.md)를 참조하세요.

> **OpenClaw를 사용하기 위해 더 이상 Mac Mini를 살 필요가 없습니다.** 집에 쉬고 있는 노트북 하나면 충분합니다 — nilbox를 설치하고 지금 바로 안전하게 AI 에이전트를 시작하세요.

---

## 작동 방식

1. **VM 시작** — 데스크톱 앱이 플랫폼 백엔드를 통해 VM을 시작합니다 (macOS의 Apple Virtualization.framework, Linux/Windows의 QEMU).
2. **게스트 에이전트 연결** — VM 내부의 Rust 에이전트가 호스트로의 VSOCK 채널을 수립합니다.
3. **AI 에이전트의 API 호출** — 요청이 로컬 아웃바운드 프록시(`127.0.0.1:8088`)를 통해 전달됩니다.
4. **호스트 프록시 가로채기** — 신뢰된 도메인의 경우 프록시가 더미 환경변수 이름을 실제 API 토큰으로 교체합니다. 신뢰되지 않은 도메인의 경우 더미 값이 통과되거나 요청이 차단됩니다.
5. **응답 반환** — 토큰 사용량이 추출되어 구성 가능한 한도에 대해 추적됩니다.

---

<p align="center">
  <img src="docs/nilbox-screen.png" width="800" alt="nilbox 스크린샷">
</p>

---

## 기능

### 보안 및 격리

- **암호화된 키스토어** — SQLCipher + OS 키링 (macOS Keychain / Linux secret-service / Windows 기본)
- **도메인 게이팅** — 런타임에서 도메인별 한 번만 허용 / 항상 허용 / 거부
- **DNS 차단 목록** — VM 아웃바운드 트래픽용 Bloom 필터 차단 목록
- **인증 위임** — Bearer, AWS SigV4, Rhai 스크립팅 OAuth 기본 제공

### AI 에이전트 지원

- **MCP 브릿지** — 호스트와 VM 간의 Model Context Protocol 브릿징 (stdio + SSE)
- **토큰 사용량 모니터링** — 프로바이더별 추적, 구성 가능한 한도 (80% 경고, 95% 차단)
- **OAuth 스크립트 엔진** — Rhai 스크립팅을 통한 플러그인 가능한 인증

### VM 관리

- **멀티 VM** — 여러 VM 생성, 시작, 중지 및 모니터링
- **통합 터미널** — VSOCK PTY를 통한 xterm.js 셸로 실행 중인 게스트에 접속
- **포트 매핑** — 호스트-VM 포트 포워딩, 재시작 시 유지
- **SSH 게이트웨이** — 외부 도구용 호스트 측 SSH 접근
- **파일 매핑** — FUSE-over-VSOCK 공유 디렉토리
- **디스크 크기 조정** — VM 디스크 이미지 크기 조정, 부팅 시 자동 파티션 확장

### 생태계

- **[앱 스토어](https://store.nilbox.run/store)** — VM 내부에 앱과 MCP 서버를 원클릭으로 설치. Linux에 익숙하지 않은 사용자를 위해 설계 — 터미널 불필요. 커맨드 라인이 익숙하다면 스토어 없이 셸에서 직접 설치 가능.

---

## 문서

| 문서 | 내용 |
|------|------|
| [개발 가이드](docs/development.md) | 프로젝트 구조, 기술 스택, 플랫폼 지원, 빌드 지침 |
| [기여](CONTRIBUTING.md) | 개발 환경 설정, 코드 가이드라인, PR 워크플로우, 이슈 보고 |
| [제로 토큰 아키텍처](docs/zero-token-architecture.md) | 보안 모델 상세, 공격 시나리오, 방어 레이어, FAQ |
| [VM 이미지 스크립트](scripts/) | 플랫폼별 Debian 이미지 빌더 및 QEMU 바이너리 빌드 |
| [OAuth 스크립트](oauth-scripts/) | 프록시용 Rhai 기반 OAuth 프로바이더 정의 |
| [MCP 브릿지](scripts/mcp/) | Claude Desktop을 VM 호스팅 MCP 서버에 연결 |
| [Playwright CDP](scripts/playwright-mcp-hello/) | VSOCK을 통한 Chrome CDP로 Playwright MCP 실행 |
| [nilbox-vmm](nilbox-vmm/) | Apple Virtualization.framework를 사용하는 macOS VMM (Swift) |
| [nilbox-blocklist](nilbox/crates/nilbox-blocklist/README.md) | Bloom 필터 DNS 차단 목록 — 차단 목록 빌드, 업데이트, 조회 (OISD, URLhaus) |

---

## 기여

기여를 환영합니다! 개발 환경 설정, 코드 가이드라인, PR 워크플로우는 [CONTRIBUTING.md](CONTRIBUTING.md)를 참조하세요.

---

## 라이선스

GNU General Public License v3.0 — [LICENSE](LICENSE) 참조.

---

<p align="center">
  Built with
  <a href="https://tauri.app/">Tauri</a> ·
  <a href="https://react.dev/">React</a> ·
  <a href="https://github.com/rustls/rustls">rustls</a> ·
  <a href="https://xtermjs.org/">xterm.js</a> ·
  <a href="https://www.zetetic.net/sqlcipher/">SQLCipher</a> ·
  <a href="https://rhai.rs/">Rhai</a>
</p>
