const PREFIX = "ord_";
const ALPHABET = "0123456789abcdefghijklmnopqrstuvwxyz";
const SUFFIX_LENGTH = 12;

export function newOrderId(): string {
  let suffix = "";
  for (let i = 0; i < SUFFIX_LENGTH; i += 1) {
    suffix += ALPHABET[Math.floor(Math.random() * ALPHABET.length)];
  }
  return `${PREFIX}${suffix}`;
}

export function isOrderId(value: string): boolean {
  return /^ord_[0-9a-z]{12}$/.test(value);
}
