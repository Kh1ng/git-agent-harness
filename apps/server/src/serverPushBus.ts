/**
 * Server Push Bus for ordered, reliable message delivery
 * Inspired by t3code's ServerPushBus
 */

import type { ServerMessage } from '@git-agent-harness/contracts';

type Subscriber = (message: ServerMessage) => void;

class ServerPushBusImpl {
  private subscribers: Set<Subscriber> = new Set();
  private queue: ServerMessage[] = [];
  private isProcessing = false;
  
  subscribe(subscriber: Subscriber): () => void {
    this.subscribers.add(subscriber);
    
    // Return unsubscribe function
    return () => {
      this.subscribers.delete(subscriber);
    };
  }
  
  publish(message: ServerMessage): void {
    this.queue.push(message);
    this.processQueue();
  }
  
  private async processQueue(): Promise<void> {
    if (this.isProcessing) return;
    
    this.isProcessing = true;
    
    while (this.queue.length > 0) {
      const message = this.queue.shift()!;
      
      // Process all subscribers
      for (const subscriber of this.subscribers) {
        try {
          subscriber(message);
        } catch (error) {
          console.error('Error in push bus subscriber:', error);
        }
      }
    }
    
    this.isProcessing = false;
  }
  
  get subscriberCount(): number {
    return this.subscribers.size;
  }
  
  get queueLength(): number {
    return this.queue.length;
  }
  
  clear(): void {
    this.queue = [];
  }
}

const serverPushBus = new ServerPushBusImpl();

export function createServerPushBus(): {
  subscribe: (subscriber: Subscriber) => () => void;
  publish: (message: ServerMessage) => void;
  get subscriberCount(): number;
  get queueLength(): number;
  clear: () => void;
} {
  return {
    subscribe: serverPushBus.subscribe.bind(serverPushBus),
    publish: serverPushBus.publish.bind(serverPushBus),
    get subscriberCount() { return serverPushBus.subscriberCount; },
    get queueLength() { return serverPushBus.queueLength; },
    clear: serverPushBus.clear.bind(serverPushBus)
  };
}

export { ServerPushBusImpl };