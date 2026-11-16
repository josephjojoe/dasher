// runner [-x] OUT ERR TIMEOUT_S cmd...
//
// Runs cmd in its own session with stdout and stderr in OUT and ERR, and
// prints one JSON line with what it and every process it started cost:
//
//   wall, user, sys, cpu   the direct child's rusage from wait4 (its own time
//                          plus that of the children it reaped)
//   maxrss_kb              the direct child's peak RSS, from the kernel
//   tree_cpu               CPU of every process in the session, from
//                          /proc/<pid>/stat sampled every 10 ms (the last
//                          sample of each process), never below `cpu`
//   tree_pss_kb            peak of the summed PSS of the session's processes,
//                          from /proc/<pid>/smaps_rollup every 50 ms
//   tree_rss_kb            peak of their summed RSS, every 10 ms
//   nproc                  most processes alive at once; pids: all ever seen
//
// With -x the direct child is left out of the tree_* figures: it is a
// driver script, and what is being measured is the browser it starts.
#define _GNU_SOURCE
#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define MAXP 4096
static struct { int pid; double cpu; } seen[MAXP];
static int nseen;

static double now(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec + ts.tv_nsec / 1e9;
}

static int slurp(const char *path, char *buf, int cap) {
  int fd = open(path, O_RDONLY);
  if (fd < 0) return -1;
  int n = 0, r;
  while (n < cap - 1 && (r = read(fd, buf + n, cap - 1 - n)) > 0) n += r;
  close(fd);
  buf[n] = 0;
  return n;
}

// Session, utime+stime in ticks and RSS in pages of one process.
static int stat_of(int pid, int *session, long *ticks, long *rss_pages) {
  char path[64], buf[2048];
  snprintf(path, sizeof path, "/proc/%d/stat", pid);
  if (slurp(path, buf, sizeof buf) <= 0) return -1;
  char *p = strrchr(buf, ')');
  if (!p) return -1;
  // After ") ": state ppid pgrp session tty tpgid flags minflt cminflt majflt
  // cmajflt utime stime cutime cstime priority nice threads itreal start
  // vsize rss
  char state;
  int ppid, pgrp, sess;
  unsigned long ut, st;
  long rss;
  int n = sscanf(p + 2,
                 "%c %d %d %d %*d %*d %*u %*u %*u %*u %*u %lu %lu %*d %*d %*d %*d %*d %*d %*u %*u %ld",
                 &state, &ppid, &pgrp, &sess, &ut, &st, &rss);
  if (n != 7) return -1;
  *session = sess;
  *ticks = (long)(ut + st);
  *rss_pages = state == 'Z' ? 0 : rss;
  return 0;
}

static long pss_of(int pid) {
  char path[64], buf[4096];
  snprintf(path, sizeof path, "/proc/%d/smaps_rollup", pid);
  if (slurp(path, buf, sizeof buf) <= 0) return -1;
  char *p = strstr(buf, "\nPss:");
  if (!p) return -1;
  return atol(p + 5);
}

int main(int argc, char **argv) {
  int skip_root = 0, a = 1;
  if (argc > 1 && !strcmp(argv[1], "-x")) { skip_root = 1; a = 2; }
  if (argc < a + 4) { fprintf(stderr, "usage: runner [-x] OUT ERR TIMEOUT cmd...\n"); return 2; }
  const char *out = argv[a], *err = argv[a + 1];
  double timeout = atof(argv[a + 2]);
  char **cmd = argv + a + 3;
  long hz = sysconf(_SC_CLK_TCK), page_kb = sysconf(_SC_PAGESIZE) / 1024;

  struct timeval tv;
  gettimeofday(&tv, 0);
  double t_epoch = tv.tv_sec + tv.tv_usec / 1e6, t0 = now();
  pid_t child = fork();
  if (child < 0) return 3;
  if (child == 0) {
    setsid();
    int o = open(out, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    int e = open(err, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    int z = open("/dev/null", O_RDONLY);
    dup2(z, 0); dup2(o, 1); dup2(e, 2);
    close(o); close(e); close(z);
    execvp(cmd[0], cmd);
    _exit(127);
  }

  int status = 0, killed = 0, done = 0, tick = 0, nproc_max = 0, pss_ok = 1;
  long peak_rss = 0, peak_pss = 0;
  struct rusage ru;
  memset(&ru, 0, sizeof ru);
  double t_end = t0;
  while (!done) {
    // Sample every process in the child's session.
    DIR *d = opendir("/proc");
    long rss_sum = 0, pss_sum = 0;
    int alive = 0, want_pss = (tick % 5 == 0);
    if (d) {
      struct dirent *de;
      while ((de = readdir(d))) {
        if (!isdigit((unsigned char)de->d_name[0])) continue;
        int pid = atoi(de->d_name), sess;
        long ticks, rss;
        if (stat_of(pid, &sess, &ticks, &rss) < 0 || sess != child) continue;
        if (skip_root && pid == child) continue;
        alive++;
        rss_sum += rss * page_kb;
        if (want_pss && pss_ok) {
          long p = pss_of(pid);
          if (p >= 0) pss_sum += p;
        }
        int i;
        for (i = 0; i < nseen; i++) if (seen[i].pid == pid) break;
        if (i == nseen && nseen < MAXP) { seen[nseen].pid = pid; seen[nseen].cpu = 0; nseen++; }
        if (i < MAXP) {
          double c = (double)ticks / hz;
          if (c > seen[i].cpu) seen[i].cpu = c;
        }
      }
      closedir(d);
    }
    if (alive > nproc_max) nproc_max = alive;
    if (rss_sum > peak_rss) peak_rss = rss_sum;
    if (want_pss && pss_sum > peak_pss) peak_pss = pss_sum;
    tick++;

    pid_t w = wait4(child, &status, WNOHANG, &ru);
    if (w == child) { done = 1; t_end = now(); break; }
    if (!killed && now() - t0 > timeout) { kill(-child, SIGKILL); killed = 1; }
    struct timespec ts = {0, 10 * 1000 * 1000};
    nanosleep(&ts, 0);
  }
  // Whatever the child left running would hold the sandbox's CPU for the
  // next run.
  kill(-child, SIGKILL);

  double user = ru.ru_utime.tv_sec + ru.ru_utime.tv_usec / 1e6;
  double sys = ru.ru_stime.tv_sec + ru.ru_stime.tv_usec / 1e6;
  double tree = 0;
  for (int i = 0; i < nseen; i++) tree += seen[i].cpu;
  if (!skip_root && tree < user + sys) tree = user + sys;
  int code = WIFEXITED(status) ? WEXITSTATUS(status) : -WTERMSIG(status);
  printf("{\"t\": %.3f, \"wall\": %.4f, \"user\": %.4f, \"sys\": %.4f, \"cpu\": %.4f, \"maxrss_kb\": %ld, "
         "\"tree_cpu\": %.4f, \"tree_pss_kb\": %ld, \"tree_rss_kb\": %ld, \"nproc\": %d, \"pids\": %d, "
         "\"exit\": %d, \"killed\": %s, \"minflt\": %ld, \"majflt\": %ld, \"nvcsw\": %ld, \"nivcsw\": %ld}\n",
         t_epoch, t_end - t0, user, sys, user + sys, ru.ru_maxrss, tree, peak_pss, peak_rss, nproc_max, nseen,
         code, killed ? "true" : "false", ru.ru_minflt, ru.ru_majflt, ru.ru_nvcsw, ru.ru_nivcsw);
  return 0;
}
