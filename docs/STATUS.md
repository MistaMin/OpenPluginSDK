# OpenPluginSDK status, 2026-11-24

Read from `gh` and `git` (origin/main at `926ca9d`, CI green), not from agent memory.

## Merged (40 PRs)

#1–#20, #22–#39, #42. The 1.0 Beta run (Orca run `run_e3b84494f60a`) is done up to the API freeze: version 1.0.0-beta (#16), static CRT on Windows (#15), identity pinning (#20), voice-info (#17), `openplugin-port new` (#18), porting guides (#19), track info (#22), shell follow-ups on ABI 9 (#34), every audit fix (#23–#33) and the freeze (#35). Since then:

- #37 ports keep their host behaviour, plus a Linux VST3 shell, the VST3 idle-edit fix and Windows LV2 and ARM64 builds (merged by the owner on 2026-11-23).
- #42 X11 frame pacing.
- #36 GoodLookin DRI pinned by tag v0.2.1, then #38 foreign state (shell ABI 10) and #39 `--au-version`, merged in that order on 2026-11-24.

#39 was rebased onto #37: CMake derives the Audio Unit component version from `OP_VERSION` (as #37 does), and `build_plugin.sh --au-version N` (CMake `OP_AU_VERSION`) overrides it. The default is `release`, the derived number.

## Open PRs

| PR | Branch | Checks | Conflicts | Notes |
|---|---|---|---|---|
| #21 Linux VST3 proofs and CI | `linux-vst3` | see the PR | none | rebuilt on main's Linux shell (from #37): adds the twelve proofs, the editor proof under Xvfb, the validator, the `vst3-shell-linux` job, the Ardour width fix and docs |
| #40 VST3 controller state | `vst3-controller-state` | 19 pass | none | on hold for the owner: main's `3a8c9ca` (from #37) fixes the same bypassed-plug-in case another way |
| #41 this status | `MistaMin/sdk-reorg` | see the PR | none | docs only |

## Workspaces

The owner approved removing the finished workspaces on 2026-11-24. They are removed after this PR merges: the 30 merged, clean Orca workspaces, and the clean, merged `.claude/worktrees/agent-*` and `merge-staging` worktrees in the main checkout. Each one is checked again right before it is removed.

Kept:

- The four with a local change: `fix-dsp-robust`, `fix-dsp-speed` and `fix-native-crash` (each has a modified `Cargo.lock`), and `v1-voice-info` (an untracked `build-voice/` folder).
- Main checkout `/Volumes/Developer 2/Dependencies/OpenPluginSDK`: now on `fix-null-channels-clap-stepped`, which is pushed with no PR and built on the old pre-rebase stack. Untracked `graphify-out/`. Its local `vst3-controller-state` equals origin.
- `hybrideq-host-values`: now on `MistaMin/linux-frame-pacing` (#42, merged).
- `.claude/worktrees/linux-vst3`: still on the old `156d65e`. The remote branch was rebuilt, so reset it to the remote; don't merge it.
- `.claude/worktrees/GoodLookinDri`: branch `MistaMin/semibold-text`, unmerged.
- `sdk-reorg`: this map.

## Remaining, in priority order

1. #21: merge once CI is green.
2. #40: the owner decides whether to merge it alongside `3a8c9ca` or close it.
3. `fix-null-channels-clap-stepped` in the main checkout: two commits (null host channels at a block offset, CLAP stepped grids; the AAX 2048-step limit) on the old stack. Open a PR after rebasing it onto main.
4. 1.0 Beta run t10: the five API-docs tasks (`missing_docs`). Missing on `ded6d8d`: core 401, dsp 431, dri 133, targets 16, host 9 (also testing 69 and port 318, which the run does not cover).
5. 1.0 Beta run t11: the release check, last.
6. Linux VST3: the identity proof (`vst3_identity_proof`), which runs on macOS and Windows, is not yet built on Linux.
7. Owner only: tagging 1.0, the crates.io publish, publishing GoodLookin DRI, the Windows items that wait for the PC (ROADMAP_WINDOWS_LINUX.md W3), real DAW sessions (Logic, Cubase, Bitwig), PACE signing.
