const hexByte = Array.from({length:256}, (_, value) => value.toString(16).padStart(2, '0'));
const hexStart = hexByte.map(value => '#' + value);

export function cssColor(color) {
  return typeof color === 'number'
    ? hexStart[color >>> 16] + hexByte[(color >>> 8) & 255] + hexByte[color & 255]
    : color;
}

export function rgbValue(color) {
  return typeof color === 'number' ? color : Number.parseInt(color.slice(1),16);
}

export function sameCellColor(left, right) {
  return left === right || (typeof left !== typeof right && rgbValue(left) === rgbValue(right));
}
