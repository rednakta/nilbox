### 코드 리뷰 요청 사항 (Change Requests)

1. **`.await` 지점을 가로지르는 `tokio::sync::RwLockReadGuard` 유지 방지**
   - **What to change:** `c.exec_command(...)`를 `.await` 하는 동안 `instance.ssh_client.read().await` 락을 유지하지 않도록 `start_metrics_collection`의 메트릭 수집 루프를 리팩토링합니다. `SshClient`를 클론하거나 락 범위를 줄이는 전략을 고려하세요.
   - **Why it matters:** Tokio `RwLock` 읽기 락 가드를 `.await` 지점을 가로질러 유지하면 미묘한 데드락이 발생하거나 `stop_vm`과 같이 `ssh_client`에 대한 쓰기 락을 획득하려는 다른 작업들을 블록할 수 있습니다.
   - **Where to change it:** `nilbox/crates/nilbox-core/src/service.rs`, `start_metrics_collection` 내부의 `exec_result` 변수 할당 부분.

2. **JSON 파싱을 위한 스트림 파편화(Fragmentation) 처리**
   - **What to change:** `serde_json::from_str`을 호출하기 전에 `stream`에서 읽을 때 라인 버퍼(예: `tokio::io::BufReader`)나 프레이밍 코덱을 사용하여 완전한 JSON 메시지가 조립된 후 파싱하도록 변경합니다.
   - **Why it matters:** `stream.read().await`는 불완전한 바이트 청크를 반환할 수 있습니다. JSON 페이로드가 여러 청크에 걸쳐 있을 때 이를 직접 문자열로 파싱하면 에러가 발생하여, 설치 출력이 누락되거나 완료 이벤트를 놓칠 수 있습니다.
   - **Where to change it:** `nilbox/crates/nilbox-core/src/service.rs`, `store_install_app`의 백그라운드 태스크 루프 내부 (`match stream.read().await`).

3. **완료 마커에 대한 SSH 패킷 파편화 처리**
   - **What to change:** 반복(iteration) 전반에 걸쳐 상태를 유지하는 버퍼에 SSH 데이터 청크를 누적시키고, 해당 누적 버퍼 내에서 `\x01NILBOX_DONE:<exit_code>\x01` 마커를 검색하도록 합니다.
   - **Why it matters:** SSH 채널 데이터는 임의의 크기로 파편화될 수 있습니다. 마커 문자열이 두 개의 `ChannelMsg::Data` 패킷으로 나뉘어 들어올 경우, 현재의 단일 청크 기반 부분 문자열 검색 방식은 마커를 완전히 놓치게 되어 앱 설치 프로세스가 정상적으로 종료되지 못합니다.
   - **Where to change it:** `nilbox/crates/nilbox-core/src/service.rs`, `open_shell` 백그라운드 태스크 내부 `ChannelMsg::Data` 처리 부분.

4. **플랫폼별 SSH/Vsock 설정 중복 제거**
   - **What to change:** `#[cfg(target_os)]` 매크로 블록 내에는 OS별 `listener` 생성 로직만 격리시킵니다. 변수 클론 및 `tokio::spawn(async move { Self::setup_vsock_and_ssh(...) })`를 호출하는 광범위한 로직은 밖으로 빼내어 플랫폼 무관하게 공유되는 하나의 블록으로 통합합니다.
   - **Why it matters:** 동일한 설정 로직이 세 번(macOS, Windows, Linux)이나 중복 작성되어 있어 불필요하게 코드 크기가 커지며 향후 수정 시 특정 플랫폼만 누락되는 등의 버그 발생 가능성을 높입니다.
   - **Where to change it:** `nilbox/crates/nilbox-core/src/service.rs`, `start_vm` 내부.

5. **JWT 디코딩 시 `URL_SAFE_NO_PAD` 사용**
   - **What to change:** 수동으로 문자열 패딩 길이를 계산하여 추가하는 로직(`"=".repeat(...)`)을 제거하고, `base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(...)`를 사용하도록 교체합니다.
   - **Why it matters:** JWT는 기본적으로 패딩이 없는 base64url 인코딩을 사용합니다. `base64` 크레이트에서 패딩 없는 디코딩을 명시적으로 지원하므로, 수동으로 패딩을 계산하고 문자열 조작을 하는 것은 불필요하며 오류가 발생하기 쉽습니다.
   - **Where to change it:** `nilbox/crates/nilbox-core/src/service.rs`, `extract_verified_from_token` 내부.

6. **고아(Orphan) 토큰 정리 시 에러 처리 조건 세분화**
   - **What to change:** 키스토어 조회 에러가 발생했을 때, 실제로 토큰이 없음을 의미하는 에러(예: "Not Found")인지 명시적으로 확인한 후에만 해당 토큰을 고아 토큰으로 판단하여 DB에서 삭제하도록 합니다.
   - **Why it matters:** 일시적인 파일 시스템 IO, IPC, 시스템 부하 등 다른 원인에 의한 에러까지 모두 "토큰 부재"로 간주하게 되면, 멀쩡히 활성화된 도메인 토큰이 의도치 않게 영구 삭제되는 치명적인 결과가 발생할 수 있습니다.
   - **Where to change it:** `nilbox/crates/nilbox-core/src/service.rs`, `cleanup_orphan_tokens` 내부 (`match self.state.keystore.get(account).await`).