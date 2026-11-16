# SPDX-License-Identifier: MIT
# The AAX identity pinning proof (aax/host_identity.cpp): one process saves a project
# with one build of the plugin, another opens it with a build whose parameter table has
# another order. Run by ctest as
#   cmake -DPINNED_A=... -DPINNED_B=... -DPLAIN_A=... -DPLAIN_B=... -DWORK=<folder> -P identity_proof.cmake
function(step)
  execute_process(COMMAND ${ARGN} RESULT_VARIABLE result)
  if(NOT result EQUAL 0)
    message(FATAL_ERROR "AAX identity proof: ${ARGN} failed (${result})")
  endif()
endfunction()
file(MAKE_DIRECTORY "${WORK}")
# Pinned: A's project opens in B, and B's in A.
step("${PINNED_A}" save "${WORK}/pinned-a.project")
step("${PINNED_B}" open "${WORK}/pinned-a.project" same)
step("${PINNED_B}" save "${WORK}/pinned-b.project")
step("${PINNED_A}" open "${WORK}/pinned-b.project" same)
# Each build opens its own project.
step("${PINNED_A}" open "${WORK}/pinned-a.project" same)
step("${PLAIN_A}" save "${WORK}/plain-a.project")
step("${PLAIN_A}" open "${WORK}/plain-a.project" same)
# The negative control: without pins the reordered build refuses the chunk, both ways.
step("${PLAIN_B}" open "${WORK}/plain-a.project" refused)
step("${PLAIN_B}" save "${WORK}/plain-b.project")
step("${PLAIN_A}" open "${WORK}/plain-b.project" refused)
message(STATUS "AAX identity proof passed")
