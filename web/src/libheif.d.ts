declare module "libheif-js/libheif-wasm/libheif-bundle.mjs" {
  export interface HeifImage {
    get_width(): number;
    get_height(): number;
    display(
      target: { data: Uint8ClampedArray; width: number; height: number },
      done: (result: { data: Uint8ClampedArray } | null) => void,
    ): void;
    free(): void;
  }

  export interface Libheif {
    HeifDecoder: new () => { decode(bytes: Uint8Array): HeifImage[] };
  }

  const createLibheif: () => Libheif | Promise<Libheif>;
  export default createLibheif;
}
