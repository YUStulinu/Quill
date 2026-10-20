# Same work as closures.ql, with Python lists (Python's recursion limit rules out cons lists of 100 000).
xs = list(range(1, 100001))
total = 0
for k in range(50):
    ys = list(map(lambda x: x * 3 + k, xs))
    zs = list(filter(lambda y: y % 2 == 0, ys))
    acc = 0
    for b in zs:
        acc = acc + b % 1000
    total += acc
print(total)
