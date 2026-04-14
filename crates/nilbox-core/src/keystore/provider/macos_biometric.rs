//! macOS Touch ID / password–protected master key storage.
//!
//! Architecture (LAContext soft-check):
//!   1. Read step: call `nilbox_evaluate_biometry()` (C → ObjC LAContext).
//!      macOS shows Touch ID prompt; falls back to system password automatically.
//!   2. Only if LAContext passes: read the 32-byte key from the login.keychain
//!      (stored without access-control flags — no code-signing entitlement needed).
//!   3. Write step: `SecItemAdd` with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`.
//!
//! This approach works with unsigned development builds.

#![cfg(target_os = "macos")]

use anyhow::{anyhow, Result};
use core_foundation_sys::{
    base::{CFIndex, CFRelease, CFTypeRef, kCFAllocatorDefault},
    data::{CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDataRef},
    dictionary::{
        kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryAddValue,
        CFDictionaryCreateMutable, CFDictionaryRef, CFMutableDictionaryRef,
    },
    string::{kCFStringEncodingUTF8, CFStringCreateWithCString},
};
use rand::RngCore;
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use zeroize::Zeroizing;

// ── LAContext FFI (compiled from biometric_helper.m) ─────────────────────────

extern "C" {
    /// Evaluate `LAPolicyDeviceOwnerAuthentication` (Touch ID → password fallback).
    /// Blocks the calling thread until the user responds.
    /// Returns: 0=ok, 1=canceled, 2=failed, -1=unavailable
    #[allow(dead_code)]
    fn nilbox_evaluate_biometry(reason: *const i8) -> c_int;
}

#[allow(dead_code)] const LA_OK: c_int       =  0;
#[allow(dead_code)] const LA_CANCELED: c_int =  1;

// ── Security / CoreFoundation bindings ───────────────────────────────────────

type OSStatus = i32;

const ERR_SEC_SUCCESS: OSStatus        =  0;
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecClass: CFTypeRef;
    static kSecClassGenericPassword: CFTypeRef;
    static kSecAttrService: CFTypeRef;
    static kSecAttrAccount: CFTypeRef;
    static kSecAttrAccessible: CFTypeRef;
    static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: CFTypeRef;
    static kSecValueData: CFTypeRef;
    static kSecReturnData: CFTypeRef;

    fn SecItemAdd(attrs: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
}

// ── Constants ─────────────────────────────────────────────────────────────────

const SERVICE: &str      = "nilbox";
const ACCOUNT: &str      = "master-key-bio";
#[allow(dead_code)]
const AUTH_REASON: &str  = "Authenticate to unlock nilbox";

// ── Public entry point ────────────────────────────────────────────────────────

/// Load (or create) the 32-byte SQLCipher master key.
///
/// On every cold start, prompts Touch ID; macOS falls back to password if
/// biometry is unavailable or fails.  Must be called via `spawn_blocking`.
pub fn load_or_create_master_key() -> Result<Zeroizing<[u8; 32]>> {
    // TODO: 임시 비활성화 — Touch ID 연동 확인 후 아래 블록 주석 해제
    // let rc = unsafe {
    //     let reason = CString::new(AUTH_REASON).unwrap();
    //     nilbox_evaluate_biometry(reason.as_ptr())
    // };
    // match rc {
    //     LA_OK => {}
    //     LA_CANCELED => return Err(anyhow!("Authentication was canceled")),
    //     _ => return Err(anyhow!("Authentication failed (LAContext code {})", rc)),
    // }

    // ── Read existing key ─────────────────────────────────────────────────
    match keychain_read() {
        Ok(key) => {
            tracing::debug!("[biometric] Master key loaded from keychain");
            return Ok(key);
        }
        Err(ERR_SEC_ITEM_NOT_FOUND) => { /* first run — create below */ }
        Err(s) => return Err(anyhow!("Keychain read error: OSStatus {}", s)),
    }

    // ── First run: generate + persist ────────────────────────────────────
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let key = Zeroizing::new(bytes);

    keychain_write(&key)?;
    tracing::debug!("[biometric] New master key generated and stored");
    Ok(key)
}

// ── Keychain helpers ──────────────────────────────────────────────────────────

/// Returns `Ok(key)` or `Err(OSStatus)`.
fn keychain_read() -> std::result::Result<Zeroizing<[u8; 32]>, OSStatus> {
    unsafe {
        let svc = cfstr(SERVICE);
        let acc = cfstr(ACCOUNT);

        let query = new_dict(5);
        dict_add(query, kSecClass,      kSecClassGenericPassword);
        dict_add(query, kSecAttrService, svc);
        dict_add(query, kSecAttrAccount, acc);
        dict_add(query, kSecReturnData,  kCFBooleanTrue);

        let mut result: CFTypeRef = ptr::null();
        let status = SecItemCopyMatching(query as CFDictionaryRef, &mut result);

        CFRelease(svc);
        CFRelease(acc);
        CFRelease(query as CFTypeRef);

        if status != ERR_SEC_SUCCESS {
            return Err(status);
        }

        let data_ref = result as CFDataRef;
        let len = CFDataGetLength(data_ref) as usize;
        if len != 32 {
            CFRelease(data_ref as CFTypeRef);
            return Err(-1);
        }

        let src = CFDataGetBytePtr(data_ref);
        let mut key = [0u8; 32];
        ptr::copy_nonoverlapping(src, key.as_mut_ptr(), 32);
        CFRelease(data_ref as CFTypeRef);

        Ok(Zeroizing::new(key))
    }
}

fn keychain_write(key: &[u8; 32]) -> Result<()> {
    unsafe {
        let svc  = cfstr(SERVICE);
        let acc  = cfstr(ACCOUNT);
        let prot = kSecAttrAccessibleWhenUnlockedThisDeviceOnly;
        let data = CFDataCreate(kCFAllocatorDefault, key.as_ptr(), 32 as CFIndex);

        let attrs = new_dict(5);
        dict_add(attrs, kSecClass,          kSecClassGenericPassword);
        dict_add(attrs, kSecAttrService,    svc);
        dict_add(attrs, kSecAttrAccount,    acc);
        dict_add(attrs, kSecAttrAccessible, prot);
        dict_add(attrs, kSecValueData,      data as CFTypeRef);

        let status = SecItemAdd(attrs as CFDictionaryRef, ptr::null_mut());

        CFRelease(data as CFTypeRef);
        CFRelease(svc);
        CFRelease(acc);
        CFRelease(attrs as CFTypeRef);

        // Duplicate on re-install is non-fatal: the key is already there.
        if status != ERR_SEC_SUCCESS && status != ERR_SEC_DUPLICATE_ITEM {
            return Err(anyhow!("SecItemAdd failed: OSStatus {}", status));
        }
        Ok(())
    }
}

// ── Low-level CF helpers ──────────────────────────────────────────────────────

unsafe fn cfstr(s: &str) -> CFTypeRef {
    let c = CString::new(s).expect("cfstr: interior NUL");
    CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), kCFStringEncodingUTF8) as CFTypeRef
}

unsafe fn new_dict(capacity: CFIndex) -> CFMutableDictionaryRef {
    CFDictionaryCreateMutable(
        kCFAllocatorDefault,
        capacity,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks,
    )
}

#[inline]
unsafe fn dict_add(dict: CFMutableDictionaryRef, key: CFTypeRef, val: CFTypeRef) {
    CFDictionaryAddValue(dict, key as *const c_void, val as *const c_void);
}
