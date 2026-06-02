<p align="center">
  <img src="apps/nilbox/nilbox-title-logo.jpg" width="300" alt="nilbox Logo" style="vertical-align: middle; margin-right: 12px;">
</p>

<p align="center">
  <strong>AI 에이전트, MCP 서버, 그리고 완전히 신뢰할 수 없는 앱을 안전하게 실행하기 위한 데스크톱 샌드박스.</strong>
</p>

<p align="center">
  에이전트를 호스트 OS로부터 격리된 전용 Linux VM에서 실행합니다 — 실제 VM 격리와 <a href="#제로-토큰-아키텍처">제로 토큰 아키텍처</a>로, API 키가 에이전트에 절대 닿지 않습니다.
</p>

<p align="center">
  <a href="#nilbox란">nilbox란</a> ·
  <a href="#nilbox는-누구를-위한-것인가">누구를 위한 것인가</a> ·
  <a href="#제로-토큰-아키텍처">제로 토큰</a> ·
  <a href="#에이전트-방화벽">에이전트 방화벽</a> ·
  <a href="#무엇을-실행할-수-있나">실행 가능 항목</a> ·
  <a href="#코드-변경-불필요">코드 변경 불필요</a> ·
  <a href="#빠른-시작">빠른 시작</a> ·
  <a href="#기능">기능</a> ·
  <a href="https://docs.nilbox.run/docs/intro/">문서</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/License-GPLv3-blue.svg" alt="License: GPL v3">
  <img src="https://img.shields.io/badge/version-0.2.3-green.svg" alt="Version">
  <img src="https://img.shields.io/badge/macOS-supported-success?logo=apple" alt="macOS">
  <img src="https://img.shields.io/badge/Linux-supported-success?logo=linux&logoColor=white" alt="Linux">
  <img src="https://img.shields.io/badge/Windows-supported-success?logo=windows&logoColor=white" alt="Windows">
</p>

---

## nilbox란?

**nilbox는 AI 에이전트와 MCP 서버를 안전하게 실행하기 위한 데스크톱 앱입니다.**

호스트 OS로부터 완전히 격리된 **전용 Linux VM**에서 에이전트와 MCP 서버를 실행하며, 제로 토큰 아키텍처로 토큰 유출을 **근본부터** 차단합니다 — 실제 API 키가 에이전트에 절대 닿지 않습니다.

AI 에이전트는 셸 접근, 파일시스템 접근, 그리고 외부 API 호출이 필요합니다. 호스트 커널의 컨테이너에서 실행하는 것은 진정한 격리가 아닙니다 — 특히 그 에이전트가 실제 자격 증명을 처리할 때는 더욱 그렇습니다. nilbox는 모든 에이전트에게 완전한 가상 머신과 호스트가 제어하는 네트워크를 제공합니다.

> 누군가에게 API 키를 주지 않을 것이라면, 그 키를 그들의 코드가 실행되는 곳에 두지 마세요.

---

## nilbox는 누구를 위한 것인가

어떤 사고 모델이 자신에게 맞는지 알면 이해가 빠릅니다.

대부분의 샌드박스 플랫폼은 **그 위에 무언가를 만드는 인프라**입니다. 여러 사용자의 *AI 생성 코드*를 대규모로 실행하는 제품을 출시하려면 SDK, 컨테이너 오케스트레이션, 리소스 쿼터, 멀티테넌트 스케줄링을 갖춘 서버 사이드 플랫폼을 사용하게 됩니다. 즉, 샌드박스를 *대상으로* 코드를 작성합니다.

**nilbox는 그 반대입니다 — 눈앞의 컴퓨터에서 에이전트를 그 *안에서* 실행하는 앱입니다.** 플랫폼을 구축하는 것이 아니라, 이미 가지고 있는 에이전트를 nilbox로 가리키고 안전하게 실행합니다. 에이전트 군단을 위한 클라우드 인프라가 아니라, 에이전트를 위한 개인용 보안 공간이라고 생각하면 됩니다.

다음에 해당한다면 nilbox가 잘 맞습니다:

- **자신의 컴퓨터에서 코딩 에이전트를 실행하는 개발자** — OpenClaw, Claude Code 등을 API 키나 호스트 OS를 위험에 빠뜨리지 않고 (밤새도록도) 자율적으로 작동시킵니다.
- **터미널 없이 AI 에이전트를 써보려는 사람** — 원클릭 스토어에서 에이전트와 MCP 서버를 설치합니다. Linux 지식이 필요 없습니다.
- **실행할 것에 대해 보안에 민감한 사람** — 신뢰할 수 없는 MCP 서버, 패키지, 바이너리를 실제 시스템이 아닌 일회용 VM 안에서 평가합니다.
- **에이전트를 원격으로 실행하는 사람** — 에이전트가 집에서 샌드박스에 격리된 채로 있는 동안 채팅(Telegram, Hermes)으로 제어합니다.

여러 테넌트를 위해 수천 개의 일시적 샌드박스를 띄우는 클라우드 서비스를 운영한다면 nilbox는 **필요하지 않을 것입니다** — 그것은 서버 사이드 샌드박스 인프라의 역할입니다. nilbox는 설계상 데스크톱 우선, 단일 운영자용입니다.

---

## 제로 토큰 아키텍처

핵심 아이디어는 단순합니다: **애초에 실제 토큰을 에이전트에게 주지 않습니다.**

nilbox는 *"토큰을 어떻게 보호할까?"*라고 묻는 대신, *"애초에 주지 않으면 어떨까?"*라고 묻습니다.

**기존 방식의 한계** — 실제 토큰이 에이전트에게 곧바로 전달됩니다:

```bash
# AI 에이전트 환경변수
OPENAI_API_KEY=sk-proj-abc1234567890xyz   # 실제 토큰 — 탈취 가능
```

Docker나 샌드박스 내부라 하더라도, 프롬프트 인젝션이나 악성 의존성이 환경변수를 읽어 유출할 수 있습니다. 에이전트가 실제 값을 쥐고 있는 한 이를 막을 방법이 없습니다.

**nilbox의 방식** — 에이전트는 이름과 값이 동일한 *가짜* 토큰만 보게 됩니다:

```bash
# AI 에이전트 환경변수
OPENAI_API_KEY=OPENAI_API_KEY             # 그냥 문자열 — 공격자에게 무용지물
```

실제 토큰은 호스트에만 존재하며, 에이전트는 절대 볼 수 없습니다.

**토큰 치환 흐름:**

```
┌───────────┐  OPENAI_API_KEY   ┌─────────┐   sk-proj-real   ┌──────────┐
│ AI 에이전트│ ────────────────▶ │ nilbox  │ ───────────────▶ │   LLM    │
└───────────┘                   └─────────┘                  └──────────┘
      ▲                                                             │
      │                          응답                              │
      └─────────────────────────────────────────────────────────────┘
```

<p align="center">
  <img src="docs/zero-token.png" width="800" alt="nilbox 스크린샷">
</p>

에이전트가 API 호출을 하는 순간, nilbox 호스트 프록시가 요청을 가로채어 가짜 토큰을 실제 토큰으로 교체합니다 — 단, 신뢰된 도메인에 한해서만. 에이전트는 자신이 실제 토큰을 가졌다고 믿으며 정상적인 응답을 받습니다.

**유출되어도 안전한 이유.** 공격자가 에이전트 환경에서 토큰을 빼내더라도, 얻는 것은 `OPENAI_API_KEY`라는 의미 없는 문자열뿐입니다. 악성 코드가 이를 `attacker.evil.com`으로 보내려 하면, 프록시가 해당 도메인을 차단하거나 더미 값만 전달합니다. **실제 토큰은 절대 호스트를 벗어나지 않습니다.**

결과:
- **침해 후 키 교체 불필요** — 실제 토큰이 노출된 적 없음
- **예산 충격 없음** — 프로바이더별 지출 한도가 과다 사용을 차단
- **데이터 유출 없음** — VM은 승인한 도메인에만 접근 가능

공격 시나리오와 방어 레이어는 [제로 토큰 아키텍처](docs/zero-token-architecture.md)를 참조하세요.

---

## 에이전트 방화벽

nilbox 내부의 에이전트는 **직접적인 네트워크가 없습니다.** 모든 아웃바운드 요청은 VSOCK을 통해 VM을 떠나, 호스트 측의 **AI 에이전트를 위한 방화벽**을 거칩니다 — 이 방화벽은 에이전트와 인터넷 사이에 위치해 모든 연결을 검사하고, 사용자가 제어하는 규칙에 따라 통과 여부를 결정합니다. 방화벽이 게스트 *외부*에 있기 때문에 에이전트는 이를 끄거나 우회할 수 없습니다.

- **기본 차단(default-deny) 아웃바운드** — 에이전트는 허용한 목적지에만 접근할 수 있으며, 그 외에는 모두 차단됩니다.
- **승인을 동반한 도메인 게이팅** — 새로운 목적지가 나타나면 nilbox가 요청을 멈추고 묻습니다: **한 번만 허용 / 항상 허용 / 거부.** 예상치 못한 것은 사람이 직접 판단합니다.
- **DNS 차단 목록** — 알려진 악성 도메인(OISD, URLhaus)은 Bloom 필터 차단 목록으로 자동 차단됩니다.
- **자격 증명 방화벽** — 실제 API 키는 경계를 넘지 않습니다. 프록시가 신뢰된 도메인에 한해서만 키를 주입합니다([제로 토큰 아키텍처](#제로-토큰-아키텍처) 참조). 침해된 에이전트는 애초에 가진 적 없는 것을 유출할 수 없습니다.
- **속도 및 지출 한도** — 프로바이더별 토큰 사용량 한도(80% 경고, 95% 차단)가 폭주하거나 탈취된 에이전트의 예산 소진을 막습니다.
- **감사 추적** — 아웃바운드 활동과 토큰 사용량이 추적되어, 에이전트가 어디에 접근하려 했는지 정확히 확인할 수 있습니다.

이것이 **프롬프트 인젝션 봉쇄의 실제 모습**입니다: 에이전트가 완전히 침해되더라도, 승인한 목적지에만, 더미 자격 증명을 들고, 지출 한도 안에서만 통신할 수 있습니다 — 유출이 갈 곳이 없습니다.

<p align="center">
  <img src="docs/agent-firewall.png" width="800" alt="nilbox 스크린샷">
</p>

---

## 무엇을 실행할 수 있나

nilbox는 어떤 에이전트, MCP 서버, 알 수 없는 앱이든 — 수정 없이 — VM 내부에서 실행합니다. 흔한 구성 몇 가지:

- 🤖 **[OpenClaw](https://docs.nilbox.run/docs/intro/)** — OpenAI / Anthropic / GitHub 키와 셸 접근이 필요한 자율 AI 코딩 에이전트. 노출된 키 없이 실행하세요.
- 🔌 **Claude + MCP** — VM에 호스팅된 MCP 서버를 VSOCK을 통해 Claude Desktop에 연결 ([MCP 브릿지](scripts/mcp/)).
- 📡 **Hermes & Telegram** — 채팅 연동으로 에이전트를 원격 제어.
- 🌐 **Playwright / 브라우저 자동화** — VSOCK을 통한 Chrome CDP로 Playwright MCP 실행 ([가이드](scripts/playwright-mcp-hello/)).
- 📦 **알 수 없는 모든 앱** — 호스트를 위험에 빠뜨리지 않고 신뢰할 수 없는 바이너리와 패키지를 시험.

**코드 변경 불필요.** 에이전트는 환경변수를 읽고 마치 베어메탈에서처럼 정확히 동일하게 API 호출을 수행합니다 — 토큰 교체는 게스트 *외부*의 호스트 프록시 레이어에서 투명하게 이루어집니다.

> **에이전트를 실행하기 위해 Mac Mini를 살 필요가 없습니다.** 집에 쉬고 있는 노트북 하나면 충분합니다 — nilbox를 설치하고 지금 바로 안전하게 AI 에이전트를 시작하세요.

---

## 코드 변경 불필요

**설정하는 것은 환경변수뿐입니다. 실행하는 코드는 절대 건드리지 않습니다.**

다른 샌드박스는 라이브러리입니다: SDK를 임포트하고, 그 API로 로직을 감싸고, 샌드박스를 생성하고 코드를 실행하기 위해 그 API를 호출합니다. 이는 코드가 *내 것이어서* 변경할 수 있어야 한다는 뜻이며, SDK를 의존성으로 떠안고, 에이전트를 그에 맞게 다시 작성하고, 양쪽이 업데이트될 때마다 동기화를 유지해야 합니다.

nilbox는 정반대로 작동합니다. 에이전트, MCP 서버, 앱이 VM 내부에서 **전혀 수정 없이** 실행됩니다. 환경변수를 읽고 마치 베어메탈에서처럼 정확히 동일하게 API 호출을 수행하며, 토큰 교체와 격리는 게스트 *외부*의 호스트 프록시 레이어에서 투명하게 이루어집니다. 유일한 설정은 각 프로바이더의 환경변수를 구성하는 것뿐입니다(예: `ANTHROPIC_API_KEY=ANTHROPIC_API_KEY`) — 값은 더미 이름이며, nilbox가 신뢰된 도메인에 한해서만 실제 토큰으로 치환합니다.

**왜 중요한가:**

- **변경할 수 없는 코드도 실행** — 클로즈드 소스 에이전트, 서드파티 바이너리, 신뢰할 수 없는 패키지가 모두 그대로 작동합니다. 통합할 것이 없습니다.
- **SDK 없음, 종속 없음** — 에이전트를 벤더 API에 맞게 다시 작성하거나 업스트림 릴리스를 따라가야 하는 의존성을 떠안지 않습니다.
- **유지보수 드리프트 없음** — 에이전트가 업데이트되어도 내 쪽에서 깨지는 것이 없습니다. 샌드박스 경계가 앱 외부에 있기 때문입니다.
- **협조에 의존하지 않는 격리** — 보안이 앱이 샌드박스 API를 올바르게 호출하는 것에 의해 강제되지 않습니다. 악의적이거나 버그가 있는 앱조차 VM 경계를 벗어나거나 실제 토큰에 도달할 수 없습니다.

```
# 멀티 프로바이더 설정 — 에이전트는 이 이름들만 볼 뿐, 실제 값은 절대 보지 못합니다
ANTHROPIC_API_KEY=ANTHROPIC_API_KEY
AWS_ACCESS_KEY_ID=AWS_ACCESS_KEY_ID
AWS_SECRET_ACCESS_KEY=AWS_SECRET_ACCESS_KEY
GEMINI_API_KEY=GEMINI_API_KEY
```

---

## 빠른 시작

### 다운로드

[GitHub Releases](https://github.com/paiml/nilbox/releases)에서 플랫폼에 맞는 최신 릴리스를 받아 데스크톱 앱을 설치하고 실행하세요. 첫 실행 시 nilbox가 관리하는 Linux VM이 자동으로 준비됩니다.

단계별 설정은 [설치 가이드](https://docs.nilbox.run/docs/intro/)를 참조하세요.

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

- **[에이전트 방화벽](#에이전트-방화벽)** — 호스트 측의 기본 차단(default-deny) AI 에이전트 방화벽; 허용 목록, 승인 프롬프트, 감사 추적으로 모든 아웃바운드 동작을 통제
- **실제 VM 격리** — 워크로드가 호스트 커널의 컨테이너가 아닌 완전한 가상 머신에서 실행
- **제로 토큰 프록시** — 실제 API 키가 게스트에 진입하지 않음; 호스트 프록시가 신뢰된 도메인에 한해 토큰을 실시간으로 교체
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

## 왜 컨테이너가 아니라 VM인가

대부분의 에이전트 샌드박스는 클라우드를 위해 만들어졌습니다 — 공유 클러스터 인프라에서 컨테이너를 실행하고 격리를 호스트 커널에 의존합니다. nilbox는 다른 입장을 취합니다:

- **공유 커널이 아닌 실제 VM** — 각 워크로드가 완전한 가상 머신을 갖기 때문에, 호스트 커널에서의 컨테이너 탈출이 애초에 문제가 되지 않습니다.
- **클러스터가 아닌 내 데스크톱** — nilbox는 이미 소유한 컴퓨터에서 실행됩니다. Kubernetes도, 클라우드 비용도, 운영할 인프라도 없습니다.
- **게스트에 진입하지 않는 키** — 제로 토큰 아키텍처는 침해된 에이전트가 애초에 가진 적 없는 자격 증명을 유출할 수 없다는 의미이며, 아웃바운드 필터링에만 의존하지 않습니다.
- **SDK 통합 불필요** — 라이브러리로 만들어진 샌드박스는 코드를 그 API로 감싸야 합니다. nilbox는 기존 코드를 수정 없이 실행하며, 유일한 설정은 환경변수뿐입니다. [코드 변경 불필요](#코드-변경-불필요)를 참조하세요.
- **터미널 불필요** — 원클릭 스토어로 비개발자도 에이전트와 MCP 서버를 안전하게 설치할 수 있으며, 파워 유저는 여전히 완전한 셸을 사용할 수 있습니다.

---

## 문서

| 문서 | 내용 |
|------|------|
| [문서 사이트](https://docs.nilbox.run/docs/intro/) | 소개, 설치, 에이전트 설정 및 가이드 (English / 한국어) |
| [개발 가이드](docs/development.md) | 프로젝트 구조, 기술 스택, 플랫폼 지원, 빌드 지침 |
| [기여](CONTRIBUTING.md) | 개발 환경 설정, 코드 가이드라인, PR 워크플로우, 이슈 보고 |
| [제로 토큰 아키텍처](docs/zero-token-architecture.md) | 보안 모델 상세, 공격 시나리오, 방어 레이어, FAQ |
| [VM 이미지 스크립트](scripts/) | 플랫폼별 Debian 이미지 빌더 및 QEMU 바이너리 빌드 |
| [OAuth 스크립트](oauth-scripts/) | 프록시용 Rhai 기반 OAuth 프로바이더 정의 |
| [MCP 브릿지](scripts/mcp/) | Claude Desktop을 VM 호스팅 MCP 서버에 연결 |
| [Playwright CDP](scripts/playwright-mcp-hello/) | VSOCK을 통한 Chrome CDP로 Playwright MCP 실행 |
| [nilbox-vmm](nilbox-vmm/) | Apple Virtualization.framework를 사용하는 macOS VMM (Swift) |
| [nilbox-blocklist](crates/nilbox-blocklist/README.md) | Bloom 필터 DNS 차단 목록 — 차단 목록 빌드, 업데이트, 조회 (OISD, URLhaus) |

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
