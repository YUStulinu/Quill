#include <stdio.h>

static long long fib(long long n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }

int main(void) {
    printf("%lld\n", fib(32));
    return 0;
}
