interface CachedValue<V> {
  value: V;
  expiresAt: number;
}

/**
 * Small read-through cache for expensive, side-effect-free observations.
 * Concurrent misses share one loader promise. Rejections are never cached.
 */
export class AsyncTtlCache<K, V> {
  private readonly values = new Map<K, CachedValue<V>>();
  private readonly inFlight = new Map<K, Promise<V>>();

  constructor(
    private readonly ttlMs: number,
    private readonly now: () => number = Date.now
  ) {
    if (!Number.isFinite(ttlMs) || ttlMs < 0) {
      throw new Error('AsyncTtlCache ttlMs must be a finite non-negative number');
    }
  }

  get(key: K, load: () => Promise<V>): Promise<V> {
    const cached = this.values.get(key);
    if (cached && cached.expiresAt > this.now()) {
      return Promise.resolve(cached.value);
    }
    if (cached) {
      this.values.delete(key);
    }

    const running = this.inFlight.get(key);
    if (running) {
      return running;
    }

    const pending = Promise.resolve()
      .then(load)
      .then((value) => {
        this.values.set(key, { value, expiresAt: this.now() + this.ttlMs });
        return value;
      })
      .finally(() => {
        if (this.inFlight.get(key) === pending) {
          this.inFlight.delete(key);
        }
      });
    this.inFlight.set(key, pending);
    return pending;
  }
}
