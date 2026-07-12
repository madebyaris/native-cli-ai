# Windows Release Compatibility Plan

1. Audit the workspace for Unix-only networking, process, and filesystem assumptions.
2. Keep Unix domain sockets on Unix and add loopback TCP IPC on Windows behind target-specific implementations.
3. Preserve the public IPC API so CLI attach, serve, and ordinary interactive/run flows compile unchanged.
4. Make runtime-directory fallbacks platform-safe and update release gating for tag builds.
5. Verify formatting, linting, tests, the native release build, and a Windows target check where the local toolchain permits.
6. Commit the fix on `fix/windows-release`, push it, and open a PR to `main`.
