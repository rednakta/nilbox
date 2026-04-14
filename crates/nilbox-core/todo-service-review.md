# Code Review TODO: nilbox-core/src/service.rs

> 파일: `nilbox/crates/nilbox-core/src/service.rs` (2654줄)
> 리뷰 일자: 2026-03-21

---

## Critical (즉시 수정)

### ~~C1 — `expect()` 패닉 위험 (라인 859, 913, 960)~~ ✅ DONE
- ~~**문제**: `MitmCertAuthority::new().expect("Failed to create MITM CA")` — CA 생성 실패 시 VM 시작 중 앱 전체 패닉~~
- **수정**: `expect()` → `?` (3곳 모두 적용, 2026-03-21)

### ~~C2 — xdg-open Shell Injection (라인 1327)~~ ✅ DONE
- ~~**문제**: xdg-open 훅 스크립트에서 `$1` (파일명)이 이스케이프 없이 사용됨 → shell injection 가능~~
- **수정**: python3 one-liner를 single-quote로 분리 + `_url` 변수 경유 방식으로 중첩 쿼팅 제거 (2026-03-21)

---

## High

### ~~H1 — `delete_vm()` Race Condition (라인 268-276)~~ ✅ DONE
- ~~상태 확인(`status()`) 후 실제 `stop()` 전에 다른 스레드가 VM 시작 가능~~
- ~~원자적 상태 전환 or 락 범위 확장 필요~~
- **수정**: `platform.write()` 락을 한 번만 획득해서 `status_async()` 체크와 `stop()` 호출을 원자적으로 수행 (2026-03-21)

### ~~H2 — `resize_vm_disk()` Race Condition (라인 566)~~ ✅ DONE
- ~~`vms` 락 해제 후 파일 open 전 시점에 다른 스레드가 VM 시작 가능~~
- **수정**: `Arc<VmInstance>` 클론 후 `vms` 락 즉시 해제, `platform.write()` 락을 resize 완료까지 유지 (2026-03-21)

### ~~H3 — `inject_env_to_sessions()` 성능 (라인 1800-1808)~~ ✅ DONE
- ~~대량 세션 시 매 세션마다 전체 스크립트 `clone()` → `Arc<Vec<u8>>` 또는 `Bytes` 사용 권장~~
- **수정**: `ShellCommand::Write(Arc<Vec<u8>>)` 로 변경, `Arc::clone()` 으로 refcount 증가만 수행 (2026-03-21)

### H4 — SSH setup task spawn 3중 중복 (라인 878/931/978)
- macOS/Windows/Linux에 거의 동일한 SSH setup task spawn 코드가 3번 반복
- 공통 함수 `spawn_ssh_setup_task()` 추출 권장

### H5 — VmInstance 생성 로직 중복 (라인 136-190 / 193-261)
- `register_vm` / `create_vm` 모두 거의 동일한 `VmInstance` 생성 코드 보유
- 빌더 패턴 또는 헬퍼 함수로 통합 권장

### H6 — VmPlatform 선택 코드 중복 (라인 147-154 / 219-226)
- 플랫폼별 `Box<dyn VmPlatform>` 선택 코드가 2곳 동일하게 반복
- `fn make_vm_platform(qemu_binary_path: Option<PathBuf>) -> Box<dyn VmPlatform>` 함수로 추출

### ~~H7 — `add_file_mapping()` 입력 검증 없음 (라인 1895-1901)~~ ✅ DONE
- ~~`host_path` 존재 여부 미확인~~
- ~~`vm_mount` 절대경로 여부 미확인~~
- ~~심볼릭 링크 추적 여부 미확인~~
- **수정**: `vm_mount` 절대경로 검증, `host_path` canonicalize로 존재+접근 검증, 심볼릭 링크 해석 시 info 로그 출력 (2026-03-22)

### ~~H8 — `remove_file_mapping()` 동시 호출 충돌 (라인 1911-1979)~~ ✅ DONE
- ~~동일 mapping에 대해 두 번 연속 호출 시 background task 간 충돌 가능~~
- ~~진행 중인 제거 작업에 대한 guard 필요~~
- **수정**: `pending_mapping_removals: HashSet<i64>` guard 추가, 동시 호출 시 두 번째 호출 거부, background task 완료 시 guard 해제 (2026-03-22)

### H9 — OAuth provider env 처리 로직 중복 (라인 380-408 / 1243-1261)
- 두 곳에서 거의 동일한 OAuth env 구성 로직 반복
- 공통 헬퍼 함수로 추출

### ~~H10 — 3개 플랫폼 vsock connector 모두 import (라인 34-47)~~ ✅ DONE
- ~~조건부 컴파일 없이 모든 플랫폼의 타입이 import됨~~
- **수정**: 이미 `#[cfg(target_os = "...")]`로 플랫폼별 분리 완료 확인 (2026-03-22)

---

## Medium

| # | 라인 | 문제 |
|---|------|------|
| ~~M1~~ ✅ | 여러 곳 | ~~에러를 조용히 무시하는 `ok().flatten()`, `unwrap_or_default()` 패턴~~ — `unwrap_or_else` + `warn!` 로그 추가 (12곳, 2026-03-22) |
| ~~M2~~ ✅ | 181-186 | ~~`active_vm` read → drop → write 사이 원자성 미보장~~ — write 락만 사용하여 원자적 처리 (2026-03-22) |
| ~~M3~~ ✅ | 1649 | ~~shell session 제거 중 `cmd_rx.recv()` 경합 가능성~~ — `close_shell()`에서 중복 remove 제거, background task에 cleanup 위임 (2026-03-22) |
| ~~M4~~ ✅ | 1443-1523 | ~~metrics 수집 task: VM 삭제 후에도 계속 실행 가능~~ — `CancellationToken` 추가, stop/delete 시 cancel 호출 (2026-03-22) |
| ~~M5~~ ✅ | 1288-1294 | ~~env 주입 스크립트 내용 검증 없음~~ — env var 이름 `[A-Za-z0-9_]` 검증 추가 (2곳, 2026-03-22) |
| ~~M6~~ ✅ | 1291, 1330 | ~~`chmod \|\| true` — 권한 설정 실패 무시~~ — `\|\| true` 제거, chmod 실패 시 전체 명령 에러로 전파 (2026-03-22) |
| ~~M7~~ ✅ | 1129-1132 | ~~SSH 공개키 형식 검증 없이 게스트에 주입~~ — `ssh-rsa`/`ssh-ed25519`/`ecdsa-sha2-` 등 prefix 검증 추가 (2026-03-22) |
| ~~M8~~ ✅ | 2104-2120 | ~~`map_env_to_domain` 등 동기 메서드가 async context에서 blocking I/O 수행 가능~~ — `async fn` + `spawn_blocking` 래핑으로 blocking I/O 격리 (2026-03-22) |
| ~~M9~~ ✅ | 296-305 | ~~`list_vms()` 실패 시 공유 파일 삭제 위험~~ — 실패 시 kernel/initrd 삭제 건너뛰기 (2026-03-22) |
| ~~M10~~ ✅ | 607-622 | ~~`list_vms()`: 각 VM마다 순차 `read().await`~~ — `tokio::join!` + `futures::future::join_all()` 로 VM별/필드별 병렬 읽기 (2026-03-22) |
| ~~M11~~ ✅ | 2258 | ~~manifest SHA256 일부를 로그에 기록~~ — `.get(..16)` 안전 슬라이스로 변경, 패닉 방지 (2026-03-22) |
| ~~M12~~ ✅ | 2302-2388 | ~~`vm_id.to_string()` 후 다시 `.clone()`~~ — 불필요한 `app_name_clone.clone()` 제거 (2026-03-22) |
| ~~M13~~ ✅ | 1879-1908 | ~~VM 미실행 상태에서 `activate` 요청 시 적용 안 되지만 성공 반환~~ — `Result<bool>` 반환 (`applied` 여부), 미실행 시 info 로그 + 이벤트에 `applied: false` 포함 (2026-03-22) |
| ~~M14~~ ✅ | 628-656 | ~~`update_vm_memory()`: memory_mb 범위 검증 없음~~ — 256~65536 MB 범위 검증 추가 (2026-03-22) |
| ~~M15~~ ✅ | 659-687 | ~~`update_vm_cpus()`: cpu 개수 범위 검증 없음~~ — 1~`available_parallelism()` 범위 검증 추가 (2026-03-22) |

---

## Low

| # | 라인 | 문제 |
|---|------|------|
| ~~L1~~ ✅ | 360-364 | ~~VmFsInfo 파싱 실패 시 로그 없이 0으로 치환~~ — `unwrap_or_else` + `warn!` 로그 추가 (2026-03-22) |
| ~~L2~~ ✅ | 1517-1520 | ~~주석 처리된 metrics 로그 코드~~ — 삭제 완료 (2026-03-22) |
| ~~L3~~ ✅ | 1813 | ~~`add_mapping()`: host_port < 1024 검증 없음~~ — `host_port >= 1024` 검증 추가 (2026-03-22) |
| ~~L4~~ ✅ | 2318 | ~~`vm_id.to_string()` 불필요~~ — `get_vm(&str)` 시그니처 변경, `.to_string()` 제거 (2026-03-22) |
| ~~L5~~ ✅ | 896 | ~~`QEMU_VSOCK_PORT` 상수가 다른 파일에 정의됨~~ — Windows cfg 블록에 import 추가 (2026-03-22) |
| ~~L6~~ ✅ | 801-809 | ~~`cleanup_orphan_tokens()` 실패가 `warn` 수준으로만 처리됨~~ — `error!`로 격상 (2026-03-22) |
| ~~L7~~ ✅ | 2064 | ~~domain 파라미터 타입이 `String` / `&str` 혼재~~ — `resolve_domain_access(domain: &str)` 통일 (2026-03-22) |
| ~~L8~~ ✅ | 1020/1126/1150 | ~~타임아웃 상수 분산~~ — 파일 상단에 `LISTENER_READY_DELAY_MS`, `VMM_STARTUP_TIMEOUT_SECS`, `METRICS_INTERVAL_SECS`, `FILE_UNMOUNT_TIMEOUT_SECS` 상수 추출 (2026-03-22) |
| L9 | - | 에러 무시 후 기본값 반환 패턴이 일관되지 않음 |
| L10 | - | 파일 전체 2654줄 — 기능별 모듈 분리 고려 (vm_lifecycle.rs, file_mapping.rs 등) |

---

## 우선순위 요약

| 순서 | 항목 | 난이도 |
|------|------|--------|
| 1 | C1: `expect()` → `?` 변환 (859, 913, 960) | 쉬움 |
| 2 | C2: xdg-open 이스케이프 (1327) | 쉬움 |
| 3 | H4+H5+H6: 중복 코드 공통화 | 보통 |
| 4 | H7+M14+M15: 입력 검증 추가 | 쉬움 |
| 5 | M10: `list_vms()` 병렬화 | 보통 |
| 6 | M4: metrics task cancellation | 보통 |
| 7 | H1+H2: race condition 해결 | 어려움 |
| 8 | L10: service.rs 모듈 분리 | 어려움 |
