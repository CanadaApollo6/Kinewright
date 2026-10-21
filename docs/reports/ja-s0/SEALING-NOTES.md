# Evidence sealing identities

Run headers describe the observed working tree before its evidence was sealed;
they are not reruns of the subsequently created commit SHA. Executable source,
fixtures and oracles retain their measured identities. Generated report files
are outputs and can acquire successor bytes during sealing.

The S0-5 evidence archive preserves `first-reviewed-docs/` byte-for-byte from
0e9b91bafb050d8a780feb56c4c62db86a127da9, including its self-consistent original
integrity index. `pre-seal-derivatives/` preserves the report bytes seen by the
final native source inventories before the current seal replaced those files.
A source-inventory entry for a replaced generated report can be verified there;
other source entries match the final worktree. The current integrity catalog is
`evidence-index.json`. Earlier run IDs, source identities and failed attempts
remain historical and are never relabelled as final-source reruns.
