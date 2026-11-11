// A client-rendered React app, the way create-react-app and most bundlers
// ship one: an empty #root, one bundle, data fetched after mount.
import {
  StrictMode,
  Suspense,
  createContext,
  lazy,
  memo,
  startTransition,
  useCallback,
  useContext,
  useDeferredValue,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { createRoot } from "react-dom/client";
import { createPortal, flushSync } from "react-dom";

const Currency = createContext("USD");

function formatPrice(cents, currency) {
  return new Intl.NumberFormat("en-US", { style: "currency", currency }).format(cents / 100);
}

function productsReducer(state, action) {
  switch (action.type) {
    case "loaded":
      return {
        items: [...state.items, ...action.items],
        next: action.next,
        status: "ready",
      };
    case "loading":
      return { ...state, status: "loading" };
    case "failed":
      return { ...state, status: "error", error: action.error };
    default:
      return state;
  }
}

const Product = memo(function Product({ product }) {
  const currency = useContext(Currency);
  const id = useId();
  return (
    <li className="product" data-sku={product.sku} aria-labelledby={id}>
      <h2 id={id}>{product.name}</h2>
      <span className="price">{formatPrice(product.price, currency)}</span>
      {product.tags.length > 0 && (
        <ul className="tags">
          {product.tags.map((tag) => (
            <li key={tag}>{tag}</li>
          ))}
        </ul>
      )}
    </li>
  );
});

// Measured after layout, as many apps do for sticky headers and the like.
function Header({ count }) {
  const ref = useRef(null);
  const [height, setHeight] = useState(0);
  useLayoutEffect(() => {
    setHeight(Math.round(ref.current.getBoundingClientRect().height));
  }, []);
  return (
    <header ref={ref} data-height={height}>
      <title>{`Shop (${count})`}</title>
      <meta name="description" content="A client-rendered shop" />
      <h1>Shop</h1>
    </header>
  );
}

const Reviews = lazy(
  () =>
    new Promise((resolve) => {
      setTimeout(() => resolve({ default: ReviewsList }), 50);
    }),
);

function ReviewsList() {
  return (
    <section id="reviews">
      <p>4.8 out of 5 stars</p>
    </section>
  );
}

function Search({ items }) {
  const [query, setQuery] = useState("");
  const deferred = useDeferredValue(query);
  const matches = useMemo(
    () => items.filter((p) => p.name.toLowerCase().includes(deferred.toLowerCase())),
    [items, deferred],
  );
  return (
    <div className="search">
      <input
        id="search"
        value={query}
        placeholder="Search"
        onChange={(event) => setQuery(event.target.value)}
      />
      <output id="matches">{matches.length} matching</output>
    </div>
  );
}

function App() {
  const [state, dispatch] = useReducer(productsReducer, {
    items: [],
    next: "/compat/react-spa/api/products.json",
    status: "idle",
  });
  const [toast, setToast] = useState(null);

  const loadMore = useCallback(
    (url, signal) => {
      dispatch({ type: "loading" });
      fetch(url, { signal, headers: { Accept: "application/json" } })
        .then((response) => {
          if (!response.ok) throw new Error(`HTTP ${response.status}`);
          return response.json();
        })
        .then((page) => {
          startTransition(() => dispatch({ type: "loaded", items: page.items, next: page.next }));
        })
        .catch((error) => {
          if (error.name !== "AbortError") dispatch({ type: "failed", error: String(error) });
        });
    },
    [],
  );

  // StrictMode mounts effects twice in development only; the abort keeps
  // this correct either way.
  useEffect(() => {
    const controller = new AbortController();
    loadMore("/compat/react-spa/api/products.json", controller.signal);
    return () => controller.abort();
  }, [loadMore]);

  useEffect(() => {
    if (state.status === "ready" && state.items.length > 3) {
      flushSync(() => setToast(`${state.items.length} products loaded`));
    }
  }, [state.status, state.items.length]);

  return (
    <Currency.Provider value="USD">
      <Header count={state.items.length} />
      <main>
        {state.status === "error" && <p role="alert">{state.error}</p>}
        <Search items={state.items} />
        <ul id="products">
          {state.items.map((product) => (
            <Product key={product.sku} product={product} />
          ))}
        </ul>
        {state.status === "loading" && <p className="spinner">Loading…</p>}
        {state.next && state.status === "ready" && (
          <button type="button" className="more" onClick={() => loadMore(state.next)}>
            Load more
          </button>
        )}
        <Suspense fallback={<p>Loading reviews…</p>}>
          <Reviews />
        </Suspense>
      </main>
      {toast && createPortal(<div id="toast">{toast}</div>, document.body)}
    </Currency.Provider>
  );
}

createRoot(document.getElementById("root"), {
  onUncaughtError: (error) => console.error("react uncaught", error),
}).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
