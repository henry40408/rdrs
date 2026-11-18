// Package cache holds the in-memory caches: a small TTL map and the three
// caches built on it. They live for the life of the process; writers
// invalidate explicitly and the TTL bounds the damage of a missed call.
package cache

import (
	"container/list"
	"sync"
	"time"
)

// TTL is a bounded map whose entries expire a fixed time after insertion. When
// full, the entry inserted longest ago goes first.
type TTL[K comparable, V any] struct {
	mu       sync.Mutex
	capacity int
	ttl      time.Duration
	now      func() time.Time
	items    map[K]*list.Element
	order    *list.List // front is the oldest insertion
}

type ttlItem[K comparable, V any] struct {
	key     K
	value   V
	expires time.Time
}

// NewTTL makes a cache holding at most capacity entries. A capacity of zero
// stores nothing.
func NewTTL[K comparable, V any](capacity int, ttl time.Duration) *TTL[K, V] {
	return &TTL[K, V]{
		capacity: capacity, ttl: ttl, now: time.Now,
		items: make(map[K]*list.Element), order: list.New(),
	}
}

// Get returns the value when present and not expired.
func (c *TTL[K, V]) Get(key K) (V, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.lookup(key)
}

// Put stores the value, restarting its time to live.
func (c *TTL[K, V]) Put(key K, value V) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.store(key, value)
}

// Delete removes the key.
func (c *TTL[K, V]) Delete(key K) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.remove(key)
}

// Compute reads the current value and decides, under one lock, what to store.
// update returns the new value and whether to store it; the caller sees no
// other writer between the read and the store.
func (c *TTL[K, V]) Compute(key K, update func(old V, found bool) (V, bool)) {
	c.mu.Lock()
	defer c.mu.Unlock()
	old, found := c.lookup(key)
	if value, store := update(old, found); store {
		c.store(key, value)
	}
}

func (c *TTL[K, V]) lookup(key K) (V, bool) {
	element, found := c.items[key]
	if !found {
		var zero V
		return zero, false
	}
	item := element.Value.(*ttlItem[K, V])
	if !c.now().Before(item.expires) {
		c.removeElement(element)
		var zero V
		return zero, false
	}
	return item.value, true
}

func (c *TTL[K, V]) store(key K, value V) {
	if c.capacity <= 0 {
		return
	}
	c.remove(key)
	for len(c.items) >= c.capacity {
		c.removeElement(c.order.Front())
	}
	c.items[key] = c.order.PushBack(&ttlItem[K, V]{key: key, value: value, expires: c.now().Add(c.ttl)})
}

func (c *TTL[K, V]) remove(key K) {
	if element, found := c.items[key]; found {
		c.removeElement(element)
	}
}

func (c *TTL[K, V]) removeElement(element *list.Element) {
	delete(c.items, element.Value.(*ttlItem[K, V]).key)
	c.order.Remove(element)
}
