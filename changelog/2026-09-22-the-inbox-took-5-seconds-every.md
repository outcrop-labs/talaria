- **The inbox took ~5 seconds; every RAG call stalled 3.6s.** The embedding
  client's configured docker-internal hostname fails DNS slowly before
  falling back — and the fallback was never remembered, so EVERY embed
  (search, indexing, health probes) paid the stall. The resolved base is
  now sticky. On top: `computeAlerts` ran its eight probes serially (now
  one Promise.all + a 15s cache; 4.3s → 7ms) and three surfaces shelled
  out to `docker ps` in the same breath (now a 5s cache). Home: 4.8s →
  ~40ms warm; KB search: 36ms.
