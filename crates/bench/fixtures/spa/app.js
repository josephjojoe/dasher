// A client-rendered shop in the shape of a typical React SPA: data from an
// API, a list of components with state, effects that fetch more, a
// context, a reducer and a lazily rendered detail panel. Written with
// React.createElement so it needs no build step.
(() => {
  const { createElement: h, useState, useEffect, useReducer, useMemo, useContext, createContext, Fragment } = React;

  const CartContext = createContext(null);

  function cartReducer(state, action) {
    switch (action.type) {
      case 'add': {
        const count = (state.items[action.id] || 0) + 1;
        return { items: { ...state.items, [action.id]: count }, total: state.total + action.price };
      }
      default:
        return state;
    }
  }

  function useJson(url) {
    const [state, setState] = useState({ loading: true, data: null, error: null });
    useEffect(() => {
      let live = true;
      fetch(url)
        .then((r) => {
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          return r.json();
        })
        .then((data) => live && setState({ loading: false, data, error: null }))
        .catch((error) => live && setState({ loading: false, data: null, error: String(error) }));
      return () => { live = false; };
    }, [url]);
    return state;
  }

  const price = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' });

  function Stars({ rating }) {
    const full = Math.round(rating);
    return h('span', { className: 'stars', 'aria-label': `${rating} out of 5` },
      '★'.repeat(full) + '☆'.repeat(5 - full));
  }

  function Reviews({ id }) {
    const { loading, data } = useJson(`/api/reviews?product=${id}`);
    if (loading) return h('p', { className: 'reviews-loading' }, 'Loading reviews…');
    return h('ul', { className: 'reviews' },
      data.map((r) => h('li', { key: r.id, className: 'review' },
        h(Stars, { rating: r.rating }),
        h('strong', null, r.author),
        h('p', null, r.text))));
  }

  function ProductCard({ product, featured }) {
    const cart = useContext(CartContext);
    const [open, setOpen] = useState(featured);
    const inCart = cart.state.items[product.id] || 0;
    return h('article', { className: 'product', 'data-id': product.id },
      h('img', { src: product.image, alt: product.name, loading: 'lazy', width: 160, height: 160 }),
      h('h2', { className: 'name' }, h('a', { href: `/p/${product.id}` }, product.name)),
      h('p', { className: 'description' }, product.description),
      h('div', { className: 'meta' },
        h('span', { className: 'price' }, price.format(product.price)),
        h(Stars, { rating: product.rating }),
        product.tags.map((t) => h('span', { key: t, className: 'tag' }, t))),
      h('button', {
        type: 'button',
        onClick: () => cart.dispatch({ type: 'add', id: product.id, price: product.price }),
      }, inCart ? `In cart (${inCart})` : 'Add to cart'),
      h('button', { type: 'button', className: 'toggle', onClick: () => setOpen(!open) },
        open ? 'Hide reviews' : 'Show reviews'),
      open ? h(Reviews, { id: product.id }) : null);
  }

  function Filters({ tags, active, onChange }) {
    return h('nav', { className: 'filters' },
      ['all', ...tags].map((t) => h('button', {
        key: t,
        type: 'button',
        className: t === active ? 'active' : '',
        onClick: () => onChange(t),
      }, t)));
  }

  function App() {
    const [cart, dispatch] = useReducer(cartReducer, { items: {}, total: 0 });
    const [tag, setTag] = useState('all');
    const { loading, data, error } = useJson('/api/products');
    const tags = useMemo(() => (data ? [...new Set(data.flatMap((p) => p.tags))].sort() : []), [data]);
    const shown = useMemo(
      () => (data || []).filter((p) => tag === 'all' || p.tags.includes(tag)),
      [data, tag],
    );
    useEffect(() => {
      document.title = loading ? 'Shop' : `Shop: ${shown.length} products`;
    }, [loading, shown.length]);

    let body;
    if (loading) body = h('p', { className: 'loading' }, 'Loading…');
    else if (error) body = h('p', { className: 'error' }, error);
    else {
      body = h(Fragment, null,
        h(Filters, { tags, active: tag, onChange: setTag }),
        h('section', { className: 'products' },
          shown.map((p, i) => h(ProductCard, { key: p.id, product: p, featured: i < 10 }))));
    }
    return h(CartContext.Provider, { value: { state: cart, dispatch } },
      h('header', null,
        h('h1', null, 'Shop'),
        h('span', { className: 'cart' }, `Cart: ${price.format(cart.total)}`)),
      h('main', null, body),
      h('footer', null, h('p', null, '© 2026 Shop')));
  }

  ReactDOM.createRoot(document.getElementById('root')).render(h(App));
})();
