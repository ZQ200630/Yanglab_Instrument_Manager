/** Exact JSON value equality; object key order has no meaning on the Host wire. */
export function sameJsonValue(left,right){
 if(left===right)return true;
 if(left===null||right===null||typeof left!=='object'||typeof right!=='object'||Array.isArray(left)!==Array.isArray(right))return false;
 if(Array.isArray(left)&&left.length!==right.length)return false;
 const keys=Object.keys(left);
 return keys.length===Object.keys(right).length&&keys.every(key=>Object.hasOwn(right,key)&&sameJsonValue(left[key],right[key]));
}
