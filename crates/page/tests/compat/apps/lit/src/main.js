// A small storefront built from Lit components, the way design systems and
// many modern sites render: everything inside shadow roots, data fetched
// after load, events crossing shadow boundaries, plus a plain custom
// element in the style of GitHub's and a server-rendered (declarative)
// shadow root.
import { LitElement, css, html, nothing } from "lit";
import { repeat } from "lit/directives/repeat.js";

const money = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

class ProductCard extends LitElement {
  static properties = {
    product: { attribute: false },
    added: { type: Boolean, reflect: true },
  };

  static styles = css`
    :host { display: block; border: 1px solid #ccc; }
    :host([added]) { border-color: green; }
    .price { font-weight: bold; }
  `;

  render() {
    const { name, price, tags } = this.product;
    return html`
      <h2 part="name">${name}</h2>
      <span class="price">${money.format(price)}</span>
      <ul class="tags">${tags.map((t) => html`<li>${t}</li>`)}</ul>
      <button @click=${this.#add} ?disabled=${this.added}>${this.added ? "Added" : "Add to cart"}</button>
    `;
  }

  #add() {
    this.added = true;
    this.dispatchEvent(
      new CustomEvent("add-to-cart", { detail: this.product, bubbles: true, composed: true }),
    );
  }
}
customElements.define("product-card", ProductCard);

class CartBadge extends LitElement {
  static properties = { count: { type: Number, reflect: true } };

  constructor() {
    super();
    this.count = 0;
  }

  render() {
    return html`Cart: <b>${this.count}</b>`;
  }
}
customElements.define("cart-badge", CartBadge);

class ShopApp extends LitElement {
  static properties = {
    src: {},
    products: { state: true },
    query: { state: true },
    cart: { state: true },
  };

  static styles = css`
    header { display: flex; }
  `;

  constructor() {
    super();
    this.products = null;
    this.query = "";
    this.cart = [];
    this.addEventListener("add-to-cart", (e) => {
      this.cart = [...this.cart, e.detail.sku];
    });
  }

  connectedCallback() {
    super.connectedCallback();
    fetch(this.src)
      .then((r) => r.json())
      .then((products) => {
        this.products = products;
      });
  }

  render() {
    const shown = (this.products ?? []).filter((p) =>
      p.name.toLowerCase().includes(this.query.toLowerCase()),
    );
    return html`
      <header>
        <slot name="title"></slot>
        <cart-badge count=${this.cart.length}></cart-badge>
      </header>
      <input type="search" placeholder="Filter" .value=${this.query} @input=${(e) => (this.query = e.target.value)} />
      ${this.products === null
        ? html`<p class="loading">Loading…</p>`
        : html`<p class="count">${shown.length} of ${this.products.length} products</p>
            <section class="grid">
              ${repeat(shown, (p) => p.sku, (p) => html`<product-card .product=${p}></product-card>`)}
            </section>`}
      ${this.cart.length ? html`<p class="cart">In cart: ${this.cart.join(", ")}</p>` : nothing}
      <slot></slot>
    `;
  }
}
customElements.define("shop-app", ShopApp);

// A plain custom element with no shadow root, like GitHub's
// <relative-time>: it rewrites its own text from an attribute.
class RelativeTime extends HTMLElement {
  static observedAttributes = ["datetime"];

  attributeChangedCallback() {
    this.update();
  }

  connectedCallback() {
    this.update();
  }

  update() {
    const date = new Date(this.getAttribute("datetime"));
    if (Number.isNaN(date.getTime())) return;
    this.textContent = date.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
      timeZone: "UTC",
    });
    this.setAttribute("title", date.toISOString());
  }
}
customElements.define("relative-time", RelativeTime);

// The banner was rendered on the server; its component only hydrates it.
class PromoBanner extends HTMLElement {
  connectedCallback() {
    this.shadowRoot.querySelector("strong").dataset.hydrated = "";
  }
}
customElements.define("promo-banner", PromoBanner);
