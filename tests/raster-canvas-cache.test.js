import test from 'node:test';
import assert from 'node:assert/strict';
import {createRasterCanvasCache} from '../web/raster-canvas-cache.js';

function createDocument() {
  return {
    createElement(tag) {
      assert.equal(tag, 'canvas');
      return {
        getContext() {
          return {
            createImageData(width, height) {
              return {data: new Uint8ClampedArray(width * height * 4)};
            },
            putImageData: pixels => { this.pixels = pixels.data; },
          };
        },
      };
    },
  };
}

const raster = () => ({width: 1, height: 1, rgba: 'AQID/w=='});
const rasterCharge = 276;

test('shared byte pressure releases the least recent backing and redraws its pixels', () => {
  const get = createRasterCanvasCache(2 * rasterCharge);
  const document = createDocument();
  const first = raster(), second = raster(), third = raster();
  const previous = get(document, first), expired = get(document, second);
  assert.equal(get(document, first), previous);
  get(document, third);
  assert.equal(expired.width, 0);
  assert.equal(previous.width, 1);

  const rebuilt = get(document, second);
  assert.notEqual(rebuilt, expired);
  assert.deepEqual([...rebuilt.pixels], [1, 2, 3, 255]);
});

test('entry count and individually oversized backing both preserve rendering', () => {
  const document = createDocument();
  const first = raster(), second = raster();
  const get = createRasterCanvasCache(4 * rasterCharge, 1);
  const previous = get(document, first);
  get(document, second);
  assert.equal(previous.width, 0);

  const uncached = createRasterCanvasCache(rasterCharge - 1);
  const one = uncached(document, first);
  const two = uncached(document, first);
  assert.notEqual(one, two);
  assert.deepEqual(one.pixels, two.pixels);
});
