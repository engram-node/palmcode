# Explicit bash argument grammars

Auto-Allow uses exact option tokens and bounded positional arity.
Combined, attached, abbreviated and long option variants require approval.
Wrappers and environment assignments require approval, including nested shells.
Git always requires approval until trust and executor configuration are wired.
Rejected: allowing read-only git verbs before pinning executor configuration;
repository configuration can execute helpers even for read-only operations.
Path containment and secret parity remain incomplete; this is not S1 acceptance.
