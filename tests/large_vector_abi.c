typedef long v16di __attribute__((vector_size(16 * sizeof(long))));

static v16di replacement;

static v16di add_replacement(v16di value) {
  v16di original = value;
  value = replacement;
  return original + value;
}

int main(void) {
  v16di input;
  v16di result;

  for (int i = 0; i < 16; i++) {
    input[i] = i + 1;
    replacement[i] = 100;
  }

  result = add_replacement(input);
  for (int i = 0; i < 16; i++)
    if (result[i] != i + 101)
      return i + 1;
  return 0;
}
