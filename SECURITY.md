# Security policy

Please report vulnerabilities privately through [GitHub's private reporting](https://github.com/cpknight/nitop/security/advisories/new), not in a public issue.

Fixes go into the latest release only. nitop is maintained in spare time, so there's no guaranteed response time, but reports are taken seriously.

nitop runs without privileges, makes no network connections of its own, and only reads local system information. If `zerotier-cli` is on your `PATH`, nitop runs it to read network details from the local ZeroTier service.
