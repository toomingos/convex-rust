// ValidatorJson (the shape `v.*().json` produces) -> TypeScript source using `v`.

const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

function key(k: string): string {
  return IDENT.test(k) ? k : JSON.stringify(k);
}

function int64FromJson(b64: string): bigint {
  const bytes = Buffer.from(b64, "base64");
  if (bytes.length !== 8) throw new Error(`bad $integer literal ${b64}`);
  return bytes.readBigInt64LE(0);
}

function literal(value: any): string {
  if (value === null) return "v.null()";
  if (value && typeof value === "object" && typeof value.$integer === "string") {
    return `v.literal(${int64FromJson(value.$integer)}n)`;
  }
  if (typeof value === "string" || typeof value === "boolean") {
    return `v.literal(${JSON.stringify(value)})`;
  }
  if (typeof value === "number") return `v.literal(${JSON.stringify(value)})`;
  throw new Error(`unsupported literal validator ${JSON.stringify(value)}`);
}

export function validatorToTs(j: any): string {
  switch (j?.type) {
    case "null":
      return "v.null()";
    case "number":
      return "v.number()";
    case "bigint":
      return "v.int64()";
    case "boolean":
      return "v.boolean()";
    case "string":
      return "v.string()";
    case "bytes":
      return "v.bytes()";
    case "any":
      return "v.any()";
    case "literal":
      return literal(j.value);
    case "id":
      return `v.id(${JSON.stringify(j.tableName)})`;
    case "array":
      return `v.array(${validatorToTs(j.value)})`;
    case "record":
      return `v.record(${validatorToTs(j.keys)}, ${validatorToTs(j.values.fieldType)})`;
    case "union":
      if (j.value.length === 1) return validatorToTs(j.value[0]);
      return `v.union(${j.value.map(validatorToTs).join(", ")})`;
    case "object":
      return `v.object(${objectFields(j.value)})`;
    default:
      throw new Error(`unsupported validator type ${JSON.stringify(j?.type)}`);
  }
}

/** `{ a: v.string(), b: v.optional(v.number()) }` */
export function objectFields(fields: Record<string, { fieldType: any; optional: boolean }>): string {
  const parts = Object.entries(fields).map(([name, f]) => {
    const t = validatorToTs(f.fieldType);
    return `${key(name)}: ${f.optional ? `v.optional(${t})` : t}`;
  });
  return parts.length === 0 ? "{}" : `{ ${parts.join(", ")} }`;
}
