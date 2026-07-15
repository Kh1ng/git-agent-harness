# Review usage dogfood

This temporary change provides one bounded, harmless diff for issue #114's
post-merge backend review telemetry matrix. It is not intended to merge.

Acceptance criteria:

- Reviewers can cite this exact changed file.
- Every attempted review retains run-scoped model and usage attribution.
- Missing backend metrics remain explicitly unknown rather than zero.
