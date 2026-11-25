# 0022: Conservative unattended bash in trusted sessions

The toggle starts off and lives only in the cached session permission context.
Trust is enforced in Rust; the UI only offers it for the current trusted
workspace. Workspace/context changes, process restart and Stop clear it.
A visible banner remains while enabled.

Hard asks are revalidated before exact session/project grants. Network and
shell-wrapper commands, secret or protected metadata paths, dynamic/glob
operands, parse failures and directory mutations never gain unattended
approval. Outside-workspace paths remain denied. Ordinary literal file
mutations and safe redirections can run unattended. Unknown command effects
(scripts, build tools, interpreters) still prompt; their explicit exact-string
session grants retain the previous protocol after hard-ask revalidation.

Cut: this is deliberately narrower than blanket Ask-to-Allow for unknown
commands and recursive mutations. Their effects cannot be established by
this scanner; treating them as safe was rejected. Bash remains same-UID
execution, not confinement. This policy does not inspect the transitive
behavior of an explicitly approved program or prevent filesystem races.
