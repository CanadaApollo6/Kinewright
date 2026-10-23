# C1 native Windows timing evidence

The existing cached Windows workspace job records the three added dev-profile build commands
individually in `ja1-c1-windows-build-times-<SHA>`. Its JSON contains `head_sha`, `ci_url`,
runner/image/toolchain, cache-step reference, `commands[{command,seconds}]`,
`extra_build_minutes` (sum of measured command seconds / 60), and `timeout_minutes`.
The lead copies that artifact to local SHA-bound evidence after pushing C1; no measurement can
be part of the commit it measures. Record the actual cache hit/miss from the named cache step.

Native Windows CI and added minutes are NOT RUN locally. The timeout stays at 60 minutes until
the lead compares a preceding comparable run, actual added times and normal variance. There is
no observed basis to raise it in this local commit. A timeout fails the native gate. C8 repeats
the decision after adding its shipping host link. Linux cross-checks cannot replace this evidence.
