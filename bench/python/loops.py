total = 0
i = 0
while i < 30000000:
    total = total + i % 7 * 3
    i = i + 1
print(total)
longest = 0
best = 0
n = 1
while n < 300000:
    x = n
    steps = 0
    while x != 1:
        if x % 2 == 0:
            x = x // 2
        else:
            x = 3 * x + 1
        steps = steps + 1
    if steps > longest:
        longest = steps
        best = n
    n = n + 1
print(best)
