import test from 'node:test';
import assert from 'node:assert/strict';
import {directionalPaneNeighbor as neighbor, paneRectangles, retainedPaneTree} from '../web/pane-geometry.js';

test('legacy pairs translate by arrangement and keep epoch order, even out of roster order',()=>{
  const rows=retainedPaneTree([9,3],'vertical');
  rows.share=300;
  assert.deepEqual([...paneRectangles(rows)],[
    [9,{x:0,y:0,width:1,height:.3}], [3,{x:0,y:.3,width:1,height:.7}],
  ]);
  assert.equal(neighbor(rows,3,'up'),9);
  assert.equal(neighbor(rows,9,'right'),null);
  const columns=retainedPaneTree([8,2],'horizontal');
  columns.share=800;
  const right=paneRectangles(columns).get(2);
  assert.equal(right.x,.8);assert.equal(right.y,0);assert.equal(right.height,1);
  assert.equal(neighbor(columns,8,'right'),2);
  assert.equal(neighbor(columns,2,'up'),null);
});

test('three-leaf geometry keeps L order at equal shares, selects real neighbors with asymmetric shares',()=>{
  const tree=retainedPaneTree([9,5,8],'vertical',true);
  assert.equal(neighbor(tree,9,'right'),5);
  assert.equal(neighbor(tree,8,'up'),5);
  assert.equal(neighbor(tree,5,'left'),9);
  assert.equal(neighbor(tree,4,'up'),null); // Parked epoch never enters layout.
  tree.share=300;tree.second.share=200;
  assert.deepEqual(paneRectangles(tree).get(5),{x:.3,y:0,width:.7,height:.2});
  assert.deepEqual(paneRectangles(tree).get(8),{x:.3,y:.2,width:.7,height:.8});
  assert.equal(neighbor(tree,9,'right'),8);
  assert.equal(neighbor(tree,8,'right'),null);
});

test('geometry rejects duplicates, unbounded recursion and shares outside core bounds',()=>{
  assert.equal(retainedPaneTree([2,2],'vertical'),null);
  const tree=retainedPaneTree([2,9],'horizontal');
  for(const share of [100,900]){tree.share=share;assert.equal(paneRectangles(tree).size,2);}
  for(const share of [99,901,NaN,100.5]){tree.share=share;assert.equal(paneRectangles(tree),null);}
  tree.share=500;tree.first=tree;assert.equal(paneRectangles(tree),null);
});
