LEAF = None


def make(depth):
    return (LEAF, LEAF) if depth == 0 else (make(depth - 1), make(depth - 1))


def check(t):
    return 0 if t is LEAF else 1 + check(t[0]) + check(t[1])


max_depth = 18
long_lived = make(max_depth)
depth = 4
while depth <= max_depth:
    iterations = 2 ** (max_depth - depth + 4)
    total = 0
    for _ in range(iterations):
        total += check(make(depth))
    print(f"{iterations} trees of depth {depth} check: {total}")
    depth += 2
print(f"long lived tree of depth {max_depth} check: {check(long_lived)}")
