# OpenPluginSDK status, 2026-11-20

Read from `gh` and `git` (origin/main at `ded6d8d`, CI green), not from agent memory.

## Merged (35 PRs)

#1–#20, #22–#35. The 1.0 Beta run (Orca run `run_e3b84494f60a`) is done up to the API freeze: version 1.0.0-beta (#16), static CRT on Windows (#15), identity pinning (#20), voice-info (#17), `openplugin-port new` (#18), porting guides (#19), track info (#22), shell follow-ups on ABI 9 (#34), every audit fix (#23–#33) and the freeze (#35).

## Open PRs

None of these came from the 1.0 Beta run. #21 and #37 were tested before the freeze renamed `openplugin-ffi` to `openplugin-host`.

| PR | Branch | Behind main | Checks | Conflicts | Notes |
|---|---|---|---|---|---|
| #21 Linux VST3 shell | `linux-vst3` | 63 | green (old) | none reported | needs a rebase and a re-run after the rename |
| #36 GoodLookin DRI v0.2.1 pin | `pin-goodlookindri-0.2.1` | 0 (rebased) | 19 pass | none | base of the stack below |
| #37 ports keep host behaviour | `MistaMin/hybrideq-host-values` | 53 | 7 failing | **yes** | rebase onto the freeze |
| #38 foreign state (ABI 10) | `foreign-state-shells` | 0 (rebased) | 19 pass | none | on top of #36 |
| #39 `--au-version` | `au-component-version` | 0 (rebased) | 19 pass | none | on top of #38 |
| #40 VST3 controller state | `vst3-controller-state` | 0 (rebased) | 19 pass | none | on top of #39 |

No PR has a review requested or a blocking review.

Why #38–#40 failed: #38 adds `op_shell_load_foreign_state` and `op_shell_foreign_kinds` but not to the C++ test stand-ins, so the VST3, AU, universal and validator jobs failed to link; #39 and #40 only inherited it. Main's #34 had meanwhile taken ABI 9. Fixed on 2026-11-20: the stack is rebased onto `ded6d8d`, foreign state is ABI 10, the stand-ins get refuse-all defaults in `native/include/stub_identity.h`, and the new test follows the frozen API. Locally the workspace tests, clippy 1.100.0, the 1.85.0 check and all 44 native proofs (VST3, AU, AAX) pass.

The main checkout still has the old `vst3-controller-state` (`0f50066`) checked out. Its remote branch is now `cb90a2c`, so reset that checkout to the remote, never merge it.

## Workspaces that can be removed (merged and clean), for the owner to approve

Orca workspaces under `~/orca/workspaces/OpenPluginSDK/`:
`0.3-a-shell-proofs`, `0.3-c-sample-rate`, `0.3-d-clap-fuzz`, `0.3-e-lv2-fuzz`, `0.3-f-shell-fuzz`, `0.3-g-state-fuzz`, `0.3-h-clap-no-alloc`, `0.3-i-lv2-shell-no-alloc`, `0.3-j-macos-ci`, `audit-dsp`, `audit-editor`, `audit-native`, `audit2-core`, `audit2-port`, `fix-aax-au`, `fix-clap-events`, `fix-dsp-precision`, `fix-editor`, `fix-lv2-state`, `fix-native-shells`*, `fix-state-gate`, `fix-x11-flaky`, `v1-api-freeze`, `v1-identity`, `v1-port-guides`, `v1-port-new`, `v1-shell-followups`*, `v1-track-info`, `v1-version`, `v1-win-crt`.

Merged, with only a regenerable change: `fix-dsp-robust`, `fix-dsp-speed`, `fix-native-crash` (modified `Cargo.lock`), `v1-voice-info` (untracked `build-voice/` build folder).

\* still has an idle agent terminal open.

In the main checkout: 47 subagent worktrees under `.claude/worktrees/agent-*` and `.claude/worktrees/merge-staging` (branch `staging`), all clean with nothing unmerged.

## Workspaces with unmerged work (keep)

- Main checkout `/Volumes/Developer 2/Dependencies/OpenPluginSDK`: branch `vst3-controller-state` (#36–#40 stack), untracked `graphify-out/` (37 MB, not for git).
- `hybrideq-host-values`: #37.
- `.claude/worktrees/linux-vst3`: #21.
- `sdk-reorg`: this map (branch `MistaMin/sdk-reorg`).

## Remaining, in priority order

1. The #36 → #38 → #39 → #40 stack: rebased, all 19 checks pass on each; merge in order (#36 first). Merging is the owner's step.
2. #21: rebase after the rename and re-run CI before merging.
3. #37: resolve its conflicts with the freeze and fix its 7 failing jobs.
4. 1.0 Beta run t10: the five API-docs tasks (`missing_docs` for core, dri, host, dsp, targets). They are ready and wait for the owner's go.
5. 1.0 Beta run t11: the release check, last.
6. `docs/ROADMAP.md` is out of date: "Remaining 1.0 work" still lists porting scaffolding (#18) and the static CRT (#15), both merged, and the 1.0 Beta table says ABI 8.
7. Owner only: tagging 1.0, the crates.io publish, publishing GoodLookin DRI, the Windows items that wait for the PC (ROADMAP_WINDOWS_LINUX.md W3), real DAW sessions (Logic, Cubase, Bitwig), PACE signing.
