#define _XOPEN_SOURCE 700
#include <ucontext.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
static ucontext_t m, c; static char *stack; static size_t size = 1 << 16; static long n = 200000;
static void body(void) { for (long i = 0; i < n; i++) swapcontext(&c, &m); }
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec * 1e-9; }
int main(void) {
  stack = malloc(size); getcontext(&c); c.uc_stack.ss_sp = stack; c.uc_stack.ss_size = size; c.uc_link = &m; makecontext(&c, body, 0);
  double t0 = now(); for (long i = 0; i < n; i++) swapcontext(&m, &c); double t1 = now();
  printf("switch round trip: %.0f ns\n", (t1 - t0) / n * 1e9);
  char *snap = malloc(size); t0 = now(); for (long i = 0; i < n; i++) { memcpy(snap, stack, 4096); } t1 = now();
  printf("snapshot of a 4 KiB live stack: %.0f ns\n", (t1 - t0) / n * 1e9);
  return 0;
}
