// usage: rus OUTFILE cmd...  runs cmd, writes "user sys maxrss_kib exit" of it to OUTFILE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>
#include <sys/resource.h>
static pid_t child;
static void fwd(int s) { if (child > 0) kill(child, s); }
int main(int argc, char **argv) {
  if (argc < 3) return 2;
  child = fork();
  if (child == 0) { execvp(argv[2], argv + 2); _exit(127); }
  signal(SIGTERM, fwd); signal(SIGINT, fwd);
  int st; struct rusage ru;
  while (wait4(child, &st, 0, &ru) < 0) {}
  FILE *f = fopen(argv[1], "w");
  int code = WIFEXITED(st) ? WEXITSTATUS(st) : -WTERMSIG(st);
  fprintf(f, "%ld.%06ld %ld.%06ld %ld %d\n", (long)ru.ru_utime.tv_sec, (long)ru.ru_utime.tv_usec,
          (long)ru.ru_stime.tv_sec, (long)ru.ru_stime.tv_usec, ru.ru_maxrss, code);
  fclose(f);
  return 0;
}
