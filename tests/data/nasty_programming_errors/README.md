# Adversarial bioinformatics provenance fixtures

Synthetic data only. NEVER execute the notebooks.

Every notebook is an independent test case. Paths are relative to the fixture
root; existing output files are intentionally small placeholders, not valid H5AD.
The parser should not inspect scientific payloads for this test.

`expectations.json` specifies expected links and forbidden targets.
An unbalanced operation with an existing target is evidence of a *possible*
historical operation, not proof of execution. Broken operations referencing
nonexistent targets should neither link nor create missing-output warnings.
Balanced operations with missing targets may legitimately create unresolved
warnings; case 18 intentionally does not forbid that warning.

The difficult recovery tests are 02, 03, 06, 15 and 17: independently recognize
subsequent READ/WRITE operations in the SAME cell, despite an earlier unclosed
call or string. 08/09/10/16 are false-positive traps.

19/20 contain invalid notebook JSON/UTF-8; the *directory scan* should continue
past them and preserve links from other notebooks. They are separate tests.

Tests are designed to FAIL where functionality is missing. No production parser
implementation is included. An expected link must have the correct direction.
