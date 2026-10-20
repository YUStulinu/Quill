#include <stdio.h>

int main(void) {
    long long total = 0;
    for (long long i = 0; i < 30000000; i++) total = total + i % 7 * 3;
    printf("%lld\n", total);
    long long longest = 0, best = 0;
    for (long long n = 1; n < 300000; n++) {
        long long x = n, steps = 0;
        while (x != 1) {
            if (x % 2 == 0) x = x / 2; else x = 3 * x + 1;
            steps++;
        }
        if (steps > longest) { longest = steps; best = n; }
    }
    printf("%lld\n", best);
    return 0;
}
