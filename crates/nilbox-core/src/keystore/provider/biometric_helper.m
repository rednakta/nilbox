// biometric_helper.m — Synchronous LAContext wrapper for Rust FFI
//
// Calls LAContext.evaluatePolicy(deviceOwnerAuthentication) which:
//   1. Shows Touch ID / Face ID prompt first
//   2. Automatically falls back to system password if biometry fails / unavailable
//
// Blocks the calling thread via dispatch_semaphore until the user responds.
// Must be called from a non-main thread (Tokio spawn_blocking).

#import <LocalAuthentication/LocalAuthentication.h>

// Return codes
#define BIOMETRIC_OK         0
#define BIOMETRIC_CANCELED   1
#define BIOMETRIC_FAILED     2
#define BIOMETRIC_UNAVAILABLE -1

int nilbox_evaluate_biometry(const char *reason_cstr) {
    dispatch_semaphore_t sema = dispatch_semaphore_create(0);
    __block int result = BIOMETRIC_UNAVAILABLE;

    LAContext *ctx = [[LAContext alloc] init];

    // deviceOwnerAuthentication = Touch ID first, then system password fallback.
    // (Use deviceOwnerAuthenticationWithBiometrics for Touch ID only, no fallback.)
    [ctx evaluatePolicy:LAPolicyDeviceOwnerAuthentication
        localizedReason:[NSString stringWithUTF8String:reason_cstr]
                  reply:^(BOOL success, NSError *error) {
        if (success) {
            result = BIOMETRIC_OK;
        } else {
            switch (error.code) {
                case LAErrorUserCancel:
                case LAErrorAppCancel:
                case LAErrorSystemCancel:
                    result = BIOMETRIC_CANCELED;
                    break;
                case LAErrorAuthenticationFailed:
                    result = BIOMETRIC_FAILED;
                    break;
                default:
                    result = BIOMETRIC_UNAVAILABLE;
                    break;
            }
        }
        dispatch_semaphore_signal(sema);
    }];

    dispatch_semaphore_wait(sema, DISPATCH_TIME_FOREVER);
    return result;
}
