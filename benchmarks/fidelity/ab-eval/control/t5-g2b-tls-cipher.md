# Source Document Bundle — TLS Cipher Suite Policy
# G2: 2 documents, different authors and terminology, no cross-references

---

## Document 1: Security Hardening Standard — TLS Configuration (2023-08-20)

Minimum protocol version: TLS 1.2. Approved cipher suites:

  TLS_RSA_WITH_AES_128_CBC_SHA256
  TLS_RSA_WITH_AES_256_CBC_SHA

These suites are industry-standard, supported by all customer environments
evaluated during the audit, and comply with current vendor guidance.
Annual review scheduled for August 2024. Decision owner: Security Team.

---

## Document 2: External Security Assessment — Findings Report (2024-09-05)

**Client**: [redacted]  **Assessor**: Vantage Security Consulting

**Finding SEC-04 (Severity: High)**

TLS endpoints are configured with static RSA key exchange (observed cipher
suite families: AES-128-CBC, AES-256-CBC). Static RSA does not provide
forward secrecy. An adversary who later obtains the server's private key
can retroactively decrypt all previously recorded TLS sessions.

NIST SP 800-52 Rev. 2 (2024 update) now designates static RSA key exchange
as insufficient for new deployments. ECDHE or DHE key agreement is required
to achieve Perfect Forward Secrecy.

**Recommendation**: Migrate to PFS-capable cipher suites within 90 days.

**Status**: Unmitigated as of assessment date.
