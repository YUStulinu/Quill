def sieve(n):
    composite = [False] * (n + 1)
    count = 0
    i = 2
    while i <= n:
        if not composite[i]:
            count += 1
            j = i * i
            while j <= n:
                composite[j] = True
                j += i
        i += 1
    return count


c = 0
for _ in range(10):
    c = sieve(2000000)
print(c)
