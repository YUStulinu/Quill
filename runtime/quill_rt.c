/*
 * The Quill run-time library: memory allocation, a mark-and-sweep garbage collector, and the built-in
 * functions. It is compiled and linked with every Quill program.
 *
 * Values are 64-bit words. An integer n is stored as 2n+1 (lowest bit 1); everything else is a pointer to
 * an 8-byte-aligned object whose first word is a header:
 *
 *     bits 0-7   kind: 0 = fields (constructors, tuples), 1 = string bytes, 2 = array, 3 = closure
 *     bits 8-31  constructor tag
 *     bits 32-63 size: number of fields, or number of bytes for strings
 *
 * Memory comes from 64 KiB pages, each holding objects of one size class (large objects get pages of
 * their own). A hash table maps page addresses to page descriptors, so the collector can tell whether an
 * arbitrary word points at a live object.
 *
 * Roots are found by scanning the stack conservatively: every word on the stack that points at an object
 * keeps it alive. Integers can never be mistaken for pointers (their lowest bit is 1), and the compiler
 * keeps every value that is live across a call either on the stack or in a callee-saved register, which
 * the collector spills to the stack before scanning. Objects themselves are scanned precisely.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <time.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <malloc.h>
#include <io.h>
#include <fcntl.h>
#else
#include <pthread.h>
#include <signal.h>
#include <unistd.h>
#endif

typedef int64_t qv;
typedef uint64_t u64;

#define KIND_FIELDS 0
#define KIND_STRING 1
#define KIND_ARRAY 2
#define KIND_CLOSURE 3

#define HDR(p) (*(u64 *)(p))
#define KIND(h) ((h) & 0xFF)
#define TAG(h) (((h) >> 8) & 0xFFFFFF)
#define SIZE(h) ((h) >> 32)
#define MAKE_HDR(kind, tag, size) ((u64)(kind) | ((u64)(tag) << 8) | ((u64)(size) << 32))
#define FIELDS(p) ((qv *)(p) + 1)
#define BYTES(p) ((char *)(p) + 8)

#define INT(n) (((qv)(n) << 1) | 1)
#define UNINT(v) ((v) >> 1)
#define UNIT INT(0)
#define BOOL(b) INT((b) ? 1 : 0)

extern qv quill_main(void);

/* ------------------------------------------------------------------------------------------------ */
/* Output                                                                                           */

static void out_write(const char *s, size_t n) { fwrite(s, 1, n, stdout); }

__attribute__((noreturn)) static void die(const char *fmt, const char *a, long long b, long long c) {
    fflush(stdout);
    fprintf(stderr, "quill: ");
    fprintf(stderr, fmt, a, b, c);
    fprintf(stderr, "\n");
    fflush(stderr);
    exit(1);
}

/* ------------------------------------------------------------------------------------------------ */
/* Pages and the page table                                                                         */

#define PAGE_SHIFT 16
#define PAGE_SIZE ((size_t)1 << PAGE_SHIFT)

typedef struct Page {
    char *start;
    size_t bytes;         /* total bytes (a multiple of PAGE_SIZE for large objects) */
    uint32_t obj_bytes;   /* object size */
    uint32_t nobjs;       /* objects in the page (1 for large pages) */
    uint32_t nwords;      /* 64-bit words in each bitmap */
    uint32_t cursor;      /* where the search for a free object resumes */
    uint32_t live;        /* objects allocated */
    uint32_t inv;         /* ceil(2^32 / obj_bytes), to divide by multiplying */
    int large;
    int cls;
    int on_list;          /* on its size class's list of pages with free objects */
    u64 *alloc_bits;
    u64 *mark_bits;
    struct Page *next;    /* next page of the same size class, or in the large-object list */
} Page;

static Page **table;            /* open addressing: key = address >> PAGE_SHIFT */
static size_t table_cap, table_len;

static size_t hash_key(uintptr_t k) { return (size_t)((k * 0x9E3779B97F4A7C15ull) >> 17); }

static void table_put(uintptr_t key, Page *p);

/* All pages, so that the table can be rebuilt and the heap swept. */
static Page **all_pages;
static size_t all_len, all_cap;

static void all_add(Page *p) {
    if (all_len == all_cap) {
        all_cap = all_cap ? all_cap * 2 : 256;
        all_pages = realloc(all_pages, all_cap * sizeof(Page *));
        if (!all_pages) die("out of memory (page list)", "", 0, 0);
    }
    all_pages[all_len++] = p;
}

#define TOMBSTONE ((Page *)1)

static void table_rebuild(size_t cap) {
    size_t need = 0;
    for (size_t i = 0; i < all_len; i++) need += all_pages[i]->bytes / PAGE_SIZE;
    while (need * 2 > cap) cap *= 2;
    free(table);
    table_cap = cap;
    table = calloc(table_cap, sizeof(Page *));
    if (!table) die("out of memory (page table)", "", 0, 0);
    table_len = 0;
    for (size_t i = 0; i < all_len; i++) {
        Page *p = all_pages[i];
        for (size_t off = 0; off < p->bytes; off += PAGE_SIZE) table_put(((uintptr_t)p->start + off) >> PAGE_SHIFT, p);
    }
}

/* Makes room for `units` more entries; call before registering a page that is already in all_pages. */
static void table_reserve(size_t units) {
    if ((table_len + units) * 2 > table_cap) {
        size_t cap = table_cap ? table_cap : 1024;
        while ((table_len + units) * 2 > cap) cap *= 2;
        table_rebuild(cap);
    }
}

static void table_put(uintptr_t key, Page *p) {
    size_t mask = table_cap - 1;
    size_t i = hash_key(key) & mask;
    while (table[i] && table[i] != TOMBSTONE) i = (i + 1) & mask;
    table[i] = p;
    table_len++;
}

static Page *table_get(uintptr_t key) {
    if (!table_cap) return NULL;
    size_t mask = table_cap - 1;
    size_t i = hash_key(key) & mask;
    for (;;) {
        Page *p = table[i];
        if (!p) return NULL;
        if (p != TOMBSTONE) {
            uintptr_t start = (uintptr_t)p->start >> PAGE_SHIFT;
            uintptr_t end = ((uintptr_t)p->start + p->bytes) >> PAGE_SHIFT;
            if (key >= start && key < end) return p;
        }
        i = (i + 1) & mask;
    }
}

static void register_page(Page *p) {
    table_reserve(p->bytes / PAGE_SIZE);
    all_add(p);
    for (size_t off = 0; off < p->bytes; off += PAGE_SIZE) table_put(((uintptr_t)p->start + off) >> PAGE_SHIFT, p);
}

static void *page_alloc(size_t bytes) {
#ifdef _WIN32
    void *m = _aligned_malloc(bytes, PAGE_SIZE);
#else
    void *m = NULL;
    if (posix_memalign(&m, PAGE_SIZE, bytes) != 0) m = NULL;
#endif
    if (!m) die("out of memory: could not allocate %s%lld bytes", "", (long long)bytes, 0);
    return m;
}

static void page_free(void *m) {
#ifdef _WIN32
    _aligned_free(m);
#else
    free(m);
#endif
}

/* ------------------------------------------------------------------------------------------------ */
/* Size classes                                                                                     */

static const uint32_t class_bytes[] = {16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256, 320, 384, 512,
                                       640, 768, 1024, 1280, 1536, 2048, 3072, 4096, 6144, 8192};
#define NCLASSES (int)(sizeof(class_bytes) / sizeof(class_bytes[0]))
#define LARGE_LIMIT 8192

/* For each class, the pages that may still have free objects; allocation takes from the first. */
static Page *class_pages[NCLASSES];

/* Size class by size in 8-byte words, filled in at start-up. */
static int8_t class_by_words[LARGE_LIMIT / 8 + 1];

static void init_classes(void) {
    int c = 0;
    for (size_t w = 0; w <= LARGE_LIMIT / 8; w++) {
        while (class_bytes[c] < w * 8) c++;
        class_by_words[w] = (int8_t)c;
    }
}

/* ------------------------------------------------------------------------------------------------ */
/* Collector state                                                                                  */

static char *stack_top;           /* the highest stack address that can hold a Quill value */
static size_t bytes_since_gc;
static size_t live_bytes;
static size_t heap_bytes;
static size_t next_gc = 8u << 20;
static long long gc_count;
static double gc_seconds;
static size_t total_allocated;
static size_t peak_heap;
static int gc_stress;
static int gc_stats;

static qv **mark_stack;
static size_t mark_len, mark_cap;

static double now_seconds(void) {
#ifdef _WIN32
    LARGE_INTEGER f, c;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&c);
    return (double)c.QuadPart / (double)f.QuadPart;
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + ts.tv_nsec / 1e9;
#endif
}

static Page *new_page_header(size_t bytes, uint32_t nobjs) {
    Page *p = calloc(1, sizeof(Page));
    if (!p) die("out of memory (page header)", "", 0, 0);
    p->start = page_alloc(bytes);
    p->bytes = bytes;
    p->nobjs = nobjs;
    p->nwords = (nobjs + 63) / 64;
    p->alloc_bits = calloc(p->nwords, sizeof(u64));
    p->mark_bits = calloc(p->nwords, sizeof(u64));
    if (!p->alloc_bits || !p->mark_bits) die("out of memory (page bitmaps)", "", 0, 0);
    heap_bytes += bytes;
    if (heap_bytes > peak_heap) peak_heap = heap_bytes;
    return p;
}

static void free_page(Page *p) {
    heap_bytes -= p->bytes;
    page_free(p->start);
    free(p->alloc_bits);
    free(p->mark_bits);
    free(p);
}

static Page *new_small_page(int cls) {
    uint32_t obj = class_bytes[cls];
    Page *p = new_page_header(PAGE_SIZE, (uint32_t)(PAGE_SIZE / obj));
    p->obj_bytes = obj;
    /* ceil(2^32 / obj): offset * inv >> 32 is offset / obj for every offset inside a page. */
    p->inv = (uint32_t)(((1ull << 32) + obj - 1) / obj);
    p->cls = cls;
    p->next = class_pages[cls];
    p->on_list = 1;
    class_pages[cls] = p;
    register_page(p);
    return p;
}

static Page *large_pages;

static void *alloc_large(size_t bytes) {
    size_t rounded = (bytes + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);
    Page *p = new_page_header(rounded, 1);
    p->obj_bytes = (uint32_t)(bytes > 0xFFFFFFFFu ? 0xFFFFFFFFu : bytes);
    p->large = 1;
    p->cls = -1;
    p->live = 1;
    p->alloc_bits[0] = 1;
    p->next = large_pages;
    large_pages = p;
    register_page(p);
    return p->start;
}

/* Is `w` the address of an allocated object? Returns its page (and index), or NULL. */
static inline Page *object_page(u64 w, uint32_t *index) {
    if (w & 7) return NULL;
    Page *p = table_get((uintptr_t)w >> PAGE_SHIFT);
    if (!p) return NULL;
    size_t off = (size_t)((char *)(uintptr_t)w - p->start);
    if (p->large) {
        if (off != 0) return NULL;
        *index = 0;
        return p;
    }
    uint32_t i = (uint32_t)(((u64)off * p->inv) >> 32);
    if ((size_t)i * p->obj_bytes != off || i >= p->nobjs) return NULL;
    if (!(p->alloc_bits[i >> 6] & (1ull << (i & 63)))) return NULL;
    *index = i;
    return p;
}

static void mark_push(qv *obj) {
    if (mark_len == mark_cap) {
        mark_cap = mark_cap ? mark_cap * 2 : 4096;
        mark_stack = realloc(mark_stack, mark_cap * sizeof(qv *));
        if (!mark_stack) die("out of memory (mark stack)", "", 0, 0);
    }
    mark_stack[mark_len++] = obj;
}

static inline void mark_word(u64 w) {
    uint32_t i;
    Page *p = object_page(w, &i);
    if (!p) return;
    u64 bit = 1ull << (i & 63);
    if (p->mark_bits[i >> 6] & bit) return;
    p->mark_bits[i >> 6] |= bit;
    mark_push((qv *)(uintptr_t)w);
}

static void mark_all(void) {
    while (mark_len) {
        qv *obj = mark_stack[--mark_len];
        u64 h = HDR(obj);
        if (KIND(h) == KIND_STRING) continue;
        u64 n = SIZE(h);
        qv *f = FIELDS(obj);
        for (u64 k = 0; k < n; k++) {
            u64 w = (u64)f[k];
            if ((w & 1) == 0) mark_word(w);
        }
    }
}

#if defined(__GNUC__)
#define NOINLINE __attribute__((noinline))
#else
#define NOINLINE
#endif

static NOINLINE void scan_stack(void) {
    char *sp = (char *)__builtin_frame_address(0);
    for (char *p = (char *)((uintptr_t)sp & ~(uintptr_t)7); p < stack_top; p += 8) {
        u64 w = *(u64 *)p;
        if ((w & 1) == 0) mark_word(w);
    }
}

/* Sweeping is a bitmap operation: an object stays allocated if it was marked. No object is visited. */
static void sweep(void) {
    live_bytes = 0;
    for (int c = 0; c < NCLASSES; c++) class_pages[c] = NULL;
    size_t keep = 0;
    int freed = 0;
    large_pages = NULL;
    for (size_t i = 0; i < all_len; i++) {
        Page *p = all_pages[i];
        uint32_t live = 0;
        for (uint32_t w = 0; w < p->nwords; w++) {
            u64 a = p->alloc_bits[w] & p->mark_bits[w];
            p->alloc_bits[w] = a;
            p->mark_bits[w] = 0;
            live += (uint32_t)__builtin_popcountll(a);
        }
        p->live = live;
        if (p->large) {
            if (!live) {
                free_page(p);
                freed = 1;
                continue;
            }
            live_bytes += p->obj_bytes;
            p->next = large_pages;
            large_pages = p;
            all_pages[keep++] = p;
            continue;
        }
        if (live == 0 && heap_bytes > (32u << 20)) {
            /* Give completely empty pages back once the heap is large. */
            free_page(p);
            freed = 1;
            continue;
        }
        live_bytes += (size_t)live * p->obj_bytes;
        p->cursor = 0;
        p->on_list = live < p->nobjs;
        if (p->on_list) {
            p->next = class_pages[p->cls];
            class_pages[p->cls] = p;
        } else {
            p->next = NULL;
        }
        all_pages[keep++] = p;
    }
    all_len = keep;
    if (freed) table_rebuild(table_cap);
}

static NOINLINE void collect(void) {
    double t0 = now_seconds();
    /* Spill the callee-saved registers (which may hold the only reference to an object) into this
       frame, so the stack scan sees them. */
#if defined(__GNUC__)
    __builtin_unwind_init();
#endif
    volatile u64 anchor = 0;
    (void)anchor;
    scan_stack();
    mark_all();
    sweep();
    gc_count++;
    bytes_since_gc = 0;
    next_gc = live_bytes * 2 > (8u << 20) ? live_bytes * 2 : (8u << 20);
    gc_seconds += now_seconds() - t0;
}

/* The first free object of a class: scans the allocation bitmap of the current page from where it left
   off, 64 objects at a time. */
static inline void *alloc_small(int cls) {
    for (;;) {
        Page *p = class_pages[cls];
        if (!p) p = new_small_page(cls);
        uint32_t i = p->cursor;
        while (i < p->nobjs) {
            uint32_t w = i >> 6;
            u64 free_bits = ~p->alloc_bits[w] & (~0ull << (i & 63));
            if (free_bits) {
                uint32_t idx = (w << 6) + (uint32_t)__builtin_ctzll(free_bits);
                if (idx >= p->nobjs) break;
                p->alloc_bits[w] |= 1ull << (idx & 63);
                p->cursor = idx + 1;
                p->live++;
                return p->start + (size_t)idx * p->obj_bytes;
            }
            i = (w + 1) << 6;
        }
        /* The page is full: take it off the list. */
        p->cursor = p->nobjs;
        class_pages[cls] = p->next;
        p->next = NULL;
        p->on_list = 0;
    }
}

/* Allocates `bytes` (a multiple of 8). The memory is NOT cleared: every caller fills in the whole object
   before the next allocation, and so before any collection can look at it. */
static void *alloc_bytes(size_t bytes) {
    total_allocated += bytes;
    bytes_since_gc += bytes;
    if (gc_stress || bytes_since_gc >= next_gc) collect();
    if (bytes > LARGE_LIMIT) return alloc_large(bytes);
    return alloc_small(class_by_words[bytes >> 3]);
}

/* Called by compiled code: a new object with `nfields` fields and the given header. */
qv quill_alloc(qv nfields, qv header) {
    size_t bytes = 8 * ((size_t)nfields + 1);
    qv *obj = alloc_bytes(bytes < 16 ? 16 : bytes);
    HDR(obj) = (u64)header;
    return (qv)(uintptr_t)obj;
}

static qv new_string(size_t len) {
    if (len > 0xFFFFFFFFu) die("string too long", "", 0, 0);
    size_t bytes = 8 + len + 1;
    bytes = (bytes + 7) & ~(size_t)7;
    qv *obj = alloc_bytes(bytes);
    HDR(obj) = MAKE_HDR(KIND_STRING, 0, len);
    return (qv)(uintptr_t)obj;
}

static qv string_from(const char *s, size_t len) {
    qv r = new_string(len);
    memcpy(BYTES(r), s, len);
    return r;
}

#define STR_LEN(v) ((size_t)SIZE(HDR(v)))

/* ------------------------------------------------------------------------------------------------ */
/* Built-in functions                                                                               */

qv quill_print(qv s) {
    out_write(BYTES(s), STR_LEN(s));
    return UNIT;
}

qv quill_println(qv s) {
    out_write(BYTES(s), STR_LEN(s));
    out_write("\n", 1);
    return UNIT;
}

qv quill_print_int(qv n) {
    char buf[32];
    int len = snprintf(buf, sizeof buf, "%lld\n", (long long)UNINT(n));
    out_write(buf, (size_t)len);
    return UNIT;
}

qv quill_int_to_string(qv n) {
    char buf[32];
    int len = snprintf(buf, sizeof buf, "%lld", (long long)UNINT(n));
    return string_from(buf, (size_t)len);
}

qv quill_string_length(qv s) { return INT((qv)STR_LEN(s)); }

qv quill_string_get(qv s, qv iv) {
    long long i = UNINT(iv);
    size_t n = STR_LEN(s);
    if (i < 0 || (size_t)i >= n) die("string index %s%lld out of bounds (length %lld)", "", i, (long long)n);
    return INT((unsigned char)BYTES(s)[i]);
}

qv quill_substring(qv s, qv startv, qv lenv) {
    long long start = UNINT(startv), len = UNINT(lenv);
    size_t n = STR_LEN(s);
    if (start < 0 || len < 0 || (size_t)start > n || (size_t)len > n - (size_t)start) {
        fflush(stdout);
        fprintf(stderr, "quill: substring(%lld, %lld) out of bounds (length %lld)\n", start, len, (long long)n);
        exit(1);
    }
    qv r = new_string((size_t)len);
    memcpy(BYTES(r), BYTES(s) + start, (size_t)len);
    return r;
}

qv quill_char_to_string(qv c) {
    long long n = UNINT(c);
    if (n < 0 || n > 255) die("byte value %s%lld out of range 0..255", "", n, 0);
    char b = (char)n;
    return string_from(&b, 1);
}

qv quill_string_to_int(qv s) {
    size_t n = STR_LEN(s);
    char *buf = malloc(n + 1);
    if (!buf) die("out of memory", "", 0, 0);
    memcpy(buf, BYTES(s), n);
    buf[n] = 0;
    char *p = buf;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    char *end;
    errno = 0;
    long long v = strtoll(p, &end, 10);
    int ok = end != p && errno == 0;
    while (ok && (*end == ' ' || *end == '\t' || *end == '\n' || *end == '\r')) end++;
    ok = ok && *end == 0 && v <= ((1ll << 62) - 1) && v >= -(1ll << 62);
    if (!ok) {
        fflush(stdout);
        fprintf(stderr, "quill: not an integer: \"%s\"\n", buf);
        exit(1);
    }
    free(buf);
    return INT(v);
}

qv quill_concat(qv a, qv b) {
    size_t la = STR_LEN(a), lb = STR_LEN(b);
    qv r = new_string(la + lb);
    /* a and b are still referenced by the caller's frame, so the allocation above cannot free them. */
    memcpy(BYTES(r), BYTES(a), la);
    memcpy(BYTES(r) + la, BYTES(b), lb);
    return r;
}

static int equal(qv a, qv b) {
    if (a == b) return 1;
    if ((a & 1) || (b & 1)) return 0;
    u64 ha = HDR(a), hb = HDR(b);
    if (KIND(ha) != KIND(hb)) return 0;
    switch (KIND(ha)) {
    case KIND_STRING:
        return SIZE(ha) == SIZE(hb) && memcmp(BYTES(a), BYTES(b), SIZE(ha)) == 0;
    case KIND_FIELDS: {
        if (TAG(ha) != TAG(hb) || SIZE(ha) != SIZE(hb)) return 0;
        qv *fa = FIELDS(a), *fb = FIELDS(b);
        for (u64 i = 0; i < SIZE(ha); i++)
            if (!equal(fa[i], fb[i])) return 0;
        return 1;
    }
    default:
        /* Arrays and closures are compared by identity. */
        return 0;
    }
}

qv quill_equal(qv a, qv b) { return BOOL(equal(a, b)); }

qv quill_array_make(qv nv, qv x) {
    long long n = UNINT(nv);
    if (n < 0) die("array_make: negative length %s%lld", "", n, 0);
    if (n > 0xFFFFFFFFll) die("array_make: length %s%lld is too large", "", n, 0);
    qv *obj = alloc_bytes(8 * ((size_t)n + 1) < 16 ? 16 : 8 * ((size_t)n + 1));
    HDR(obj) = MAKE_HDR(KIND_ARRAY, 0, n);
    for (long long i = 0; i < n; i++) FIELDS(obj)[i] = x;
    return (qv)(uintptr_t)obj;
}

/* Called by the inline bounds checks of optimized code, with untagged values. */
__attribute__((noreturn)) void quill_array_index_error(long long i, long long n) {
    die("array index %s%lld out of bounds (length %lld)", "", i, n);
}

__attribute__((noreturn)) void quill_string_index_error(long long i, long long n) {
    die("string index %s%lld out of bounds (length %lld)", "", i, n);
}

static void check_index(qv a, long long i) {
    long long n = (long long)SIZE(HDR(a));
    if (i < 0 || i >= n) die("array index %s%lld out of bounds (length %lld)", "", i, n);
}

qv quill_array_get(qv a, qv iv) {
    long long i = UNINT(iv);
    check_index(a, i);
    return FIELDS(a)[i];
}

qv quill_array_set(qv a, qv iv, qv x) {
    long long i = UNINT(iv);
    check_index(a, i);
    FIELDS(a)[i] = x;
    return UNIT;
}

qv quill_array_length(qv a) { return INT((qv)SIZE(HDR(a))); }

__attribute__((noreturn)) qv quill_panic(qv msg) {
    fflush(stdout);
    fprintf(stderr, "quill: ");
    fwrite(BYTES(msg), 1, STR_LEN(msg), stderr);
    fprintf(stderr, "\n");
    exit(1);
}

__attribute__((noreturn)) void quill_panic_div0(void) { die("division by zero%s", "", 0, 0); }

__attribute__((noreturn)) void quill_unreachable(void) { die("internal error: reached unreachable code%s", "", 0, 0); }

qv quill_assert(qv c) {
    if (c == BOOL(0)) die("assertion failed%s", "", 0, 0);
    return UNIT;
}

static double start_time;

qv quill_clock_ms(void) { return INT((qv)((now_seconds() - start_time) * 1000.0)); }

qv quill_gc_collect(void) {
    collect();
    return UNIT;
}

qv quill_gc_count(void) { return INT(gc_count); }

/* ------------------------------------------------------------------------------------------------ */
/* Entry point                                                                                      */

/* Deep non-tail recursion can exhaust the stack. Report it like any other run-time error instead of
   dying silently. The handler runs on reserved stack space (Windows) or an alternate stack (POSIX). */
static const char overflow_msg[] = "quill: stack overflow (recursion too deep)\n";
#define STACK_SIZE ((size_t)1 << 30)

#ifdef _WIN32
static LONG WINAPI on_exception(EXCEPTION_POINTERS *e) {
    if (e->ExceptionRecord->ExceptionCode != EXCEPTION_STACK_OVERFLOW) return EXCEPTION_CONTINUE_SEARCH;
    fflush(stdout);
    DWORD written;
    WriteFile(GetStdHandle(STD_ERROR_HANDLE), overflow_msg, sizeof overflow_msg - 1, &written, NULL);
    ExitProcess(1);
}

static void install_overflow_handler(void) {
    ULONG guarantee = 64 * 1024;
    SetThreadStackGuarantee(&guarantee);
    AddVectoredExceptionHandler(1, on_exception);
}
#else
static void on_segv(int sig, siginfo_t *info, void *ctx) {
    (void)ctx;
    char *addr = (char *)info->si_addr;
    /* Only faults just below the program's stack are overflows; anything else is a real crash. */
    if (stack_top && addr < stack_top && addr >= stack_top - STACK_SIZE - ((size_t)1 << 20)) {
        fflush(stdout);
        ssize_t r = write(2, overflow_msg, sizeof overflow_msg - 1);
        (void)r;
        _exit(1);
    }
    signal(sig, SIG_DFL);
    raise(sig);
}

static void install_overflow_handler(void) {
    static char alt[64 * 1024];
    stack_t ss = {0};
    ss.ss_sp = alt;
    ss.ss_size = sizeof alt;
    sigaltstack(&ss, NULL);
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = on_segv;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
}
#endif

static NOINLINE void run(void) {
    /* Everything the program puts on the stack lives below this frame. */
    stack_top = (char *)__builtin_frame_address(0);
    install_overflow_handler();
    quill_main();
}

#ifdef _WIN32
static DWORD WINAPI thread_main(LPVOID arg) {
    (void)arg;
    run();
    return 0;
}
#else
static void *thread_main(void *arg) {
    (void)arg;
    run();
    return NULL;
}
#endif

int main(void) {
    static char outbuf[1 << 16];
#ifdef _WIN32
    /* Write "\n", not "\r\n": programs print the same bytes on every platform. */
    _setmode(_fileno(stdout), _O_BINARY);
#endif
    setvbuf(stdout, outbuf, _IOFBF, sizeof outbuf);
    const char *stress = getenv("QUILL_GC_STRESS");
    gc_stress = stress && *stress && *stress != '0';
    const char *stats = getenv("QUILL_GC_STATS");
    gc_stats = stats && *stats && *stats != '0';
    start_time = now_seconds();
    init_classes();
    /* Quill programs recurse deeply; run them on a thread with a 1 GiB stack (reserved, not committed). */
    size_t stack = STACK_SIZE;
#ifdef _WIN32
    HANDLE t = CreateThread(NULL, stack, thread_main, NULL, STACK_SIZE_PARAM_IS_A_RESERVATION, NULL);
    if (!t) die("could not start the main thread%s", "", 0, 0);
    WaitForSingleObject(t, INFINITE);
#else
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, stack);
    pthread_t t;
    if (pthread_create(&t, &attr, thread_main, NULL) != 0) die("could not start the main thread%s", "", 0, 0);
    pthread_join(t, NULL);
#endif
    fflush(stdout);
    if (gc_stats) {
        fprintf(stderr, "gc: %lld collections, %.1f ms in total, %.1f MiB allocated, peak heap %.1f MiB\n", gc_count,
                gc_seconds * 1000.0, total_allocated / 1048576.0, peak_heap / 1048576.0);
    }
    return 0;
}
