#define _XOPEN_SOURCE 700
#include <ucontext.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static ucontext_t main_ctx, co_ctx, snap_ctx;
static char *stack; static size_t size = 1 << 16;
static char *snap; static size_t snap_len; static int shots = 0;
static void yield_(void) { swapcontext(&co_ctx, &main_ctx); }
static void body(void) {
  int local = 40;            /* lives on the coroutine stack */
  yield_();                  /* capture point: the scheduler snapshots here */
  local += 2;
  printf("shot %d sees local=%d\n", ++shots, local);
  swapcontext(&co_ctx, &main_ctx);
}
int main(void) {
  stack = malloc(size);
  getcontext(&co_ctx); co_ctx.uc_stack.ss_sp = stack; co_ctx.uc_stack.ss_size = size; co_ctx.uc_link = &main_ctx;
  makecontext(&co_ctx, body, 0);
  swapcontext(&main_ctx, &co_ctx);          /* runs to the capture point */
  snap = malloc(size); memcpy(snap, stack, size); snap_ctx = co_ctx; /* snapshot: stack bytes + context */
  swapcontext(&main_ctx, &co_ctx);          /* first resumption */
  memcpy(stack, snap, size); co_ctx = snap_ctx;                    /* restore in place */
  swapcontext(&main_ctx, &co_ctx);          /* second resumption of the same continuation */
  puts(shots == 2 ? "multi-shot by in-place restore: ok" : "FAILED");
  return 0;
}
