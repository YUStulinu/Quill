def digits_sum(s):
    total = 0
    for ch in s:
        total += ord(ch) - 48
    return total


acc = 0
for i in range(1000000):
    s = str(i) + "-" + str(i * 7)
    acc += len(s) + digits_sum(str(i))
print(acc)
