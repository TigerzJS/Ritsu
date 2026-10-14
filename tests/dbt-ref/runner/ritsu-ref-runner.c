// ritsu-ref-runner: executes DBT differential-test vectors on a real AArch64 CPU (or qemu-user).
//
// Static, freestanding aarch64-Linux program: no libc, raw syscalls only, so the result does not
// depend on the distribution of the machine it runs on (docs/process/testing.md section 4.3).
// File formats and the execution model are documented in tests/dbt-ref/README.md.
//
// Per vector the runner
//   1. fills the data window from the vector's memory seed and loads the code page:
//        [ prologue (256 bytes) ][ test words ][ BRK sentinels ]
//      The prologue is position independent and loads X0-X30, SP and NZCV from the vector;
//      it falls straight through into the test words, which therefore start at `code_base`.
//   2. flushes the instruction cache, makes the page executable (mprotect) and jumps to it,
//   3. receives the end of the block as a signal (the BRK sentinel -> SIGTRAP, or SIGILL/SIGSEGV/
//      SIGBUS/SIGFPE/SIGTRAP raised by the test words themselves). The signal handler records
//      X0-X30, SP, NZCV and PC from the signal context and redirects the sigreturn to
//      `resume_point`, which restores the runner's callee-saved registers.
//
// Build: see tests/dbt-ref/runner/build.sh.

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long u64;
typedef long i64;

#define RUNNER_VERSION 1u

#define PROLOGUE_BYTES 256u
#define PAD_WORDS 8u      // BRK sentinels after the test words
#define MAX_WORDS 256u    // test words per vector
#define MAX_MEM 4096u     // data window bytes
#define CODE_REGION 0x10000ul
#define DATA_REGION 0x10000ul
#define SENTINEL 0xd43e0020u  // BRK #0xf001

// ---------------------------------------------------------------------------------------------
// Syscalls.

#define SYS_openat 56
#define SYS_close 57
#define SYS_read 63
#define SYS_write 64
#define SYS_exit_group 94
#define SYS_sigaltstack 132
#define SYS_rt_sigaction 134
#define SYS_mprotect 226
#define SYS_mmap 222
#define SYS_setitimer 103

static inline i64 sys6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f) {
  register i64 x8 __asm__("x8") = n;
  register i64 x0 __asm__("x0") = a;
  register i64 x1 __asm__("x1") = b;
  register i64 x2 __asm__("x2") = c;
  register i64 x3 __asm__("x3") = d;
  register i64 x4 __asm__("x4") = e;
  register i64 x5 __asm__("x5") = f;
  __asm__ volatile("svc 0"
                   : "+r"(x0)
                   : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
                   : "memory", "cc");
  return x0;
}

static void die(const char *msg, int code);

// ---------------------------------------------------------------------------------------------
// Minimal libc replacements (the compiler may emit calls to these).

void *memcpy(void *d, const void *s, u64 n) {
  u8 *dp = d;
  const u8 *sp = s;
  while (n--) *dp++ = *sp++;
  return d;
}

void *memset(void *d, int c, u64 n) {
  u8 *dp = d;
  while (n--) *dp++ = (u8)c;
  return d;
}

int memcmp(const void *a, const void *b, u64 n) {
  const u8 *x = a, *y = b;
  for (; n; n--, x++, y++)
    if (*x != *y) return *x < *y ? -1 : 1;
  return 0;
}

static u64 cstr_len(const char *s) {
  u64 n = 0;
  while (s[n]) n++;
  return n;
}

static int cstr_eq(const char *a, const char *b) {
  while (*a && *a == *b) a++, b++;
  return *a == *b;
}

static void write_all(int fd, const void *buf, u64 n) {
  const u8 *p = buf;
  while (n) {
    i64 r = sys6(SYS_write, fd, (i64)p, (i64)n, 0, 0, 0);
    if (r <= 0) die("ritsu-ref-runner: write failed\n", 3);
    p += r;
    n -= (u64)r;
  }
}

static void die(const char *msg, int code) {
  write_all(2, msg, cstr_len(msg));
  sys6(SYS_exit_group, code, 0, 0, 0, 0, 0);
  for (;;) {
  }
}

// ---------------------------------------------------------------------------------------------
// File formats (little endian, see README.md).

struct vec_header {
  char magic[8];  // "RITSUDV1"
  u32 version;    // 1
  u32 flags;      // 0
  u64 seed;       // master seed (informational)
  u64 code_base;  // address of the first test word
  u64 mem_base;   // address of the data window
  u32 count;      // number of vectors
  u32 mem_len;    // data window length in bytes
  u32 gen_version;
  u32 reserved[3];
};

struct vec_fixed {  // fixed part of a vector record; followed by `nwords` u32 words
  u16 nwords;
  u16 class_id;
  u32 reserved;
  u64 mem_seed;
  u64 x[31];
  u64 sp;
  u64 nzcv;  // PSTATE bits 31:28
};

struct res_header {
  char magic[8];  // "RITSURR1"
  u32 version;    // 1
  u32 runner_version;
  u32 count;
  u32 mem_len;
  u64 code_base;
  u64 mem_base;
  u64 seed;
  u8 reserved[16];
};  // 64 bytes

struct res_rec {  // followed by `mem_len` bytes of final memory if flags & 1
  u16 signal;
  u16 flags;  // bit 0: data window changed
  int si_code;
  u64 pc;
  u64 x[31];
  u64 sp;
  u64 nzcv;
};

_Static_assert(sizeof(struct vec_header) == 64, "vec_header");
_Static_assert(sizeof(struct vec_fixed) == 280, "vec_fixed");
_Static_assert(sizeof(struct res_header) == 64, "res_header");
_Static_assert(sizeof(struct res_rec) == 280, "res_rec");

// ---------------------------------------------------------------------------------------------
// Buffered input / output.

static int in_fd = 0;
static u8 in_buf[1 << 16];
static u64 in_pos, in_len;

static void read_exact(void *dst, u64 n) {
  u8 *d = dst;
  while (n) {
    if (in_pos == in_len) {
      i64 r = sys6(SYS_read, in_fd, (i64)in_buf, sizeof in_buf, 0, 0, 0);
      if (r <= 0) die("ritsu-ref-runner: truncated vector file\n", 3);
      in_pos = 0;
      in_len = (u64)r;
    }
    u64 k = in_len - in_pos;
    if (k > n) k = n;
    memcpy(d, in_buf + in_pos, k);
    in_pos += k;
    d += k;
    n -= k;
  }
}

static int out_fd = 1;
static u8 out_buf[1 << 18];
static u64 out_len;

static void flush_out(void) {
  write_all(out_fd, out_buf, out_len);
  out_len = 0;
}

static void put(const void *p, u64 n) {
  if (out_len + n > sizeof out_buf) flush_out();
  memcpy(out_buf + out_len, p, n);
  out_len += n;
}

// ---------------------------------------------------------------------------------------------
// Cache maintenance (compiler-rt's __clear_cache is not available without libc).

static void clear_cache(u64 start, u64 end) {
  u64 ctr;
  __asm__ volatile("mrs %0, ctr_el0" : "=r"(ctr));
  u64 dline = 4ul << ((ctr >> 16) & 0xf);
  u64 iline = 4ul << (ctr & 0xf);
  for (u64 a = start & ~(dline - 1); a < end; a += dline) __asm__ volatile("dc cvau, %0" ::"r"(a) : "memory");
  __asm__ volatile("dsb ish" ::: "memory");
  for (u64 a = start & ~(iline - 1); a < end; a += iline) __asm__ volatile("ic ivau, %0" ::"r"(a) : "memory");
  __asm__ volatile("dsb ish\n isb" ::: "memory");
}

// ---------------------------------------------------------------------------------------------
// Entry stub, prologue template and sigreturn target.

struct cpu_state {  // layout used by the prologue: x0-x30 at 0..240, sp at 248, nzcv at 256
  u64 x[31];
  u64 sp;
  u64 nzcv;
};

extern char prologue_begin[], prologue_end[];
extern void run_vector(struct cpu_state *st, void *code_page);
extern char resume_point[];
u64 saved_sp;

__asm__(
    ".text\n"
    ".balign 16\n"
    ".globl _start\n"
    "_start:\n"
    "  mov x0, sp\n"
    "  and sp, x0, #-16\n"
    "  bl start_c\n"
    "1: b 1b\n"

    // void run_vector(struct cpu_state *st /*x0*/, void *code_page /*x1*/)
    ".globl run_vector\n"
    ".balign 16\n"
    "run_vector:\n"
    "  sub sp, sp, #176\n"
    "  stp x19, x20, [sp, #0]\n"
    "  stp x21, x22, [sp, #16]\n"
    "  stp x23, x24, [sp, #32]\n"
    "  stp x25, x26, [sp, #48]\n"
    "  stp x27, x28, [sp, #64]\n"
    "  stp x29, x30, [sp, #80]\n"
    "  stp d8, d9, [sp, #96]\n"
    "  stp d10, d11, [sp, #112]\n"
    "  stp d12, d13, [sp, #128]\n"
    "  stp d14, d15, [sp, #144]\n"
    "  adrp x2, saved_sp\n"
    "  mov x3, sp\n"
    "  str x3, [x2, :lo12:saved_sp]\n"
    "  br x1\n"

    // Reached through the sigreturn of the signal handler with sp == saved_sp.
    ".globl resume_point\n"
    ".balign 16\n"
    "resume_point:\n"
    "  ldp x19, x20, [sp, #0]\n"
    "  ldp x21, x22, [sp, #16]\n"
    "  ldp x23, x24, [sp, #32]\n"
    "  ldp x25, x26, [sp, #48]\n"
    "  ldp x27, x28, [sp, #64]\n"
    "  ldp x29, x30, [sp, #80]\n"
    "  ldp d8, d9, [sp, #96]\n"
    "  ldp d10, d11, [sp, #112]\n"
    "  ldp d12, d13, [sp, #128]\n"
    "  ldp d14, d15, [sp, #144]\n"
    "  add sp, sp, #176\n"
    "  ret\n"

    // Position-independent prologue, copied to the start of the code page. x0 = &cpu_state.
    // Exactly PROLOGUE_BYTES long (padded with NOPs); the test words follow directly.
    ".balign 16\n"
    ".globl prologue_begin\n"
    "prologue_begin:\n"
    "  ldr x1, [x0, #256]\n"
    "  msr nzcv, x1\n"
    "  ldr x1, [x0, #248]\n"
    "  mov sp, x1\n"
    "  ldp x2, x3, [x0, #16]\n"
    "  ldp x4, x5, [x0, #32]\n"
    "  ldp x6, x7, [x0, #48]\n"
    "  ldp x8, x9, [x0, #64]\n"
    "  ldp x10, x11, [x0, #80]\n"
    "  ldp x12, x13, [x0, #96]\n"
    "  ldp x14, x15, [x0, #112]\n"
    "  ldp x16, x17, [x0, #128]\n"
    "  ldp x18, x19, [x0, #144]\n"
    "  ldp x20, x21, [x0, #160]\n"
    "  ldp x22, x23, [x0, #176]\n"
    "  ldp x24, x25, [x0, #192]\n"
    "  ldp x26, x27, [x0, #208]\n"
    "  ldp x28, x29, [x0, #224]\n"
    "  ldr x30, [x0, #240]\n"
    "  ldp x0, x1, [x0]\n"
    "  .rept 44\n"
    "  nop\n"
    "  .endr\n"
    ".globl prologue_end\n"
    "prologue_end:\n");

// ---------------------------------------------------------------------------------------------
// Signal handling.

#define SIGILL_ 4
#define SIGTRAP_ 5
#define SIGBUS_ 7
#define SIGFPE_ 8
#define SIGSEGV_ 11
#define SIGALRM_ 14

// aarch64 Linux ucontext: mcontext (struct sigcontext) at offset 176.
struct sigctx {
  u64 fault_address;
  u64 regs[31];
  u64 sp;
  u64 pc;
  u64 pstate;
};

struct uctx {
  u64 uc_flags;
  struct uctx *uc_link;
  void *ss_sp;
  int ss_flags;
  int ss_pad;
  u64 ss_size;
  u8 sigmask[128];
  u8 align_pad[8];
  struct sigctx mc;
} __attribute__((aligned(16)));

_Static_assert(__builtin_offsetof(struct uctx, mc) == 176, "ucontext layout");

struct kernel_sigaction {  // arm64: no sa_restorer field
  void (*handler)(int, void *, void *);
  u64 flags;
  u64 mask;
};

static struct res_rec cur_res;
static int have_result;

static void on_signal(int sig, void *info, void *ucv) {
  struct uctx *uc = ucv;
  struct res_rec *r = &cur_res;
  r->signal = (u16)sig;
  r->si_code = *(int *)((u8 *)info + 8);
  r->pc = uc->mc.pc;
  for (int i = 0; i < 31; i++) r->x[i] = uc->mc.regs[i];
  r->sp = uc->mc.sp;
  r->nzcv = uc->mc.pstate & 0xf0000000ul;
  have_result = 1;
  uc->mc.pc = (u64)resume_point;
  uc->mc.sp = saved_sp;
}

static void on_alarm(int sig, void *info, void *ucv) {
  (void)sig;
  (void)info;
  (void)ucv;
  die("ritsu-ref-runner: timeout\n", 124);
}

static u8 alt_stack[1 << 16] __attribute__((aligned(16)));

static void install_handlers(void) {
  struct {
    void *sp;
    int flags;
    int pad;
    u64 size;
  } ss = {alt_stack, 0, 0, sizeof alt_stack};
  if (sys6(SYS_sigaltstack, (i64)&ss, 0, 0, 0, 0, 0) != 0) die("ritsu-ref-runner: sigaltstack failed\n", 2);
  struct kernel_sigaction sa = {on_signal, 4ul /*SA_SIGINFO*/ | 0x08000000ul /*SA_ONSTACK*/, 0};
  static const int sigs[] = {SIGILL_, SIGTRAP_, SIGBUS_, SIGFPE_, SIGSEGV_};
  for (unsigned i = 0; i < sizeof sigs / sizeof sigs[0]; i++)
    if (sys6(SYS_rt_sigaction, sigs[i], (i64)&sa, 0, 8, 0, 0) != 0) die("ritsu-ref-runner: sigaction failed\n", 2);
  struct kernel_sigaction al = {on_alarm, 4ul | 0x08000000ul, 0};
  sys6(SYS_rt_sigaction, SIGALRM_, (i64)&al, 0, 8, 0, 0);
}

// ---------------------------------------------------------------------------------------------
// Vector execution.

static u64 splitmix_next(u64 *s) {
  *s += 0x9e3779b97f4a7c15ul;
  u64 z = *s;
  z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ul;
  z = (z ^ (z >> 27)) * 0x94d049bb133111ebul;
  return z ^ (z >> 31);
}

static void fill_window(u8 *dst, u32 len, u64 seed) {
  u64 s = seed;
  for (u32 i = 0; i < len; i += 8) {
    u64 v = splitmix_next(&s);
    for (u32 j = 0; j < 8 && i + j < len; j++) dst[i + j] = (u8)(v >> (8 * j));
  }
}

static void *map_fixed(u64 addr, u64 len) {
  // PROT_READ|PROT_WRITE = 3, MAP_PRIVATE|MAP_ANONYMOUS = 0x22. The address is a hint that must
  // be honoured exactly (no MAP_FIXED, so nothing is silently replaced).
  i64 r = sys6(SYS_mmap, (i64)addr, (i64)len, 3, 0x22, -1, 0);
  if (r != (i64)addr) die("ritsu-ref-runner: cannot map the fixed test addresses\n", 2);
  return (void *)r;
}

static const char *parse_args(int argc, char **argv, int *tmo) {
  const char *vec = 0;
  const char *out = 0;
  for (int i = 1; i < argc; i++) {
    if (cstr_eq(argv[i], "--version")) {
      write_all(1, "ritsu-ref-runner 1\n", 19);
      sys6(SYS_exit_group, 0, 0, 0, 0, 0, 0);
    } else if (cstr_eq(argv[i], "-i") && i + 1 < argc) {
      vec = argv[++i];
    } else if (cstr_eq(argv[i], "-o") && i + 1 < argc) {
      out = argv[++i];
    } else if (cstr_eq(argv[i], "-t") && i + 1 < argc) {
      const char *p = argv[++i];
      int v = 0;
      while (*p >= '0' && *p <= '9') v = v * 10 + (*p++ - '0');
      *tmo = v;
    } else {
      die("usage: ritsu-ref-runner [-i vectors.bin] [-o results.bin] [-t timeout-seconds]\n", 2);
    }
  }
  if (vec) {
    i64 fd = sys6(SYS_openat, -100, (i64)vec, 0 /*O_RDONLY*/, 0, 0, 0);
    if (fd < 0) die("ritsu-ref-runner: cannot open the vector file\n", 2);
    in_fd = (int)fd;
  }
  if (out) {
    i64 fd = sys6(SYS_openat, -100, (i64)out, 01 | 0100 | 01000 /*O_WRONLY|O_CREAT|O_TRUNC*/, 0644, 0, 0);
    if (fd < 0) die("ritsu-ref-runner: cannot open the output file\n", 2);
    out_fd = (int)fd;
  }
  return vec;
}

void start_c(long *sp) {
  int argc = (int)sp[0];
  char **argv = (char **)(sp + 1);
  int tmo = 0;
  parse_args(argc, argv, &tmo);

  if ((u64)(prologue_end - prologue_begin) != PROLOGUE_BYTES) die("ritsu-ref-runner: bad prologue size\n", 2);

  struct vec_header vh;
  read_exact(&vh, sizeof vh);
  if (memcmp(vh.magic, "RITSUDV1", 8) != 0 || vh.version != 1) die("ritsu-ref-runner: not a v1 vector file\n", 2);
  if (vh.mem_len > MAX_MEM || (vh.code_base & 0xffff) != PROLOGUE_BYTES || (vh.mem_base & 0xffff) != 0)
    die("ritsu-ref-runner: unsupported vector header\n", 2);

  install_handlers();
  if (tmo > 0) {
    struct {
      i64 isec, iusec, vsec, vusec;
    } it = {0, 0, tmo, 0};
    sys6(SYS_setitimer, 0 /*ITIMER_REAL*/, (i64)&it, 0, 0, 0, 0);
  }

  u8 *code = map_fixed(vh.code_base - PROLOGUE_BYTES, CODE_REGION);
  u8 *data = map_fixed(vh.mem_base, DATA_REGION);
  static u8 init_mem[MAX_MEM];

  struct res_header rh;
  memset(&rh, 0, sizeof rh);
  memcpy(rh.magic, "RITSURR1", 8);
  rh.version = 1;
  rh.runner_version = RUNNER_VERSION;
  rh.count = vh.count;
  rh.mem_len = vh.mem_len;
  rh.code_base = vh.code_base;
  rh.mem_base = vh.mem_base;
  rh.seed = vh.seed;
  put(&rh, sizeof rh);

  for (u32 n = 0; n < vh.count; n++) {
    static struct vec_fixed vf;
    static u32 words[MAX_WORDS];
    read_exact(&vf, sizeof vf);
    if (vf.nwords == 0 || vf.nwords > MAX_WORDS) die("ritsu-ref-runner: bad word count\n", 3);
    read_exact(words, 4ul * vf.nwords);

    // Make the code page writable, load prologue + test words + sentinels.
    sys6(SYS_mprotect, (i64)code, CODE_REGION, 3, 0, 0, 0);
    memcpy(code, prologue_begin, PROLOGUE_BYTES);
    memcpy(code + PROLOGUE_BYTES, words, 4ul * vf.nwords);
    u32 *pad = (u32 *)(code + PROLOGUE_BYTES + 4ul * vf.nwords);
    for (u32 i = 0; i < PAD_WORDS; i++) pad[i] = SENTINEL;
    u64 used = PROLOGUE_BYTES + 4ul * (vf.nwords + PAD_WORDS);
    clear_cache((u64)code, (u64)code + used);
    sys6(SYS_mprotect, (i64)code, CODE_REGION, 5 /*PROT_READ|PROT_EXEC*/, 0, 0, 0);

    fill_window(init_mem, vh.mem_len, vf.mem_seed);
    memcpy(data, init_mem, vh.mem_len);

    static struct cpu_state st;
    memcpy(st.x, vf.x, sizeof st.x);
    st.sp = vf.sp;
    st.nzcv = vf.nzcv;

    have_result = 0;
    run_vector(&st, code);
    if (!have_result) die("ritsu-ref-runner: no result recorded\n", 3);

    int changed = memcmp(data, init_mem, vh.mem_len) != 0;
    cur_res.flags = changed ? 1 : 0;
    put(&cur_res, sizeof cur_res);
    if (changed) put(data, vh.mem_len);
  }
  flush_out();
  sys6(SYS_exit_group, 0, 0, 0, 0, 0, 0);
}
