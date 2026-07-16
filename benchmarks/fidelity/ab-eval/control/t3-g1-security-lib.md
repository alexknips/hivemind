# Security Library Evaluation Notes — Q3 2024
# G1: single document, implicit disagreement (no "contested"/"disputed" label)

**Domain**: Backend Engineering
**Date**: 2024-09-20

## Meeting Notes

Attendees: Priya (Security Lead), Marcus (Backend)

Priya presented the case for adopting **go-crypt** as our primary
cryptographic primitives library. Benefits: maintained by the OpenSSL
foundation, FIPS 140-2 validated, widely adopted in financial services.

Marcus observed that go-crypt's key management API is harder to use safely
than the current library and would require significant team training. He
recalled prior industry incidents where teams misconfigured key derivation
parameters after migrating to similar APIs. He suggested completing a
prototype of the key management flow before committing.

The group agreed to gather more information. Priya will prepare a migration
cost estimate by 2024-10-04. Marcus will prototype the key management path
and document the operational burden.

## Open Items

- go-crypt migration estimate (Priya) — due 2024-10-04
- Key management prototype (Marcus) — due 2024-10-04
