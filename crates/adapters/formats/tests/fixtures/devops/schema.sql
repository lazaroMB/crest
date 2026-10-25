-- PostgreSQL schema
CREATE TABLE public.users (
    id bigserial PRIMARY KEY,
    email varchar(255) NOT NULL UNIQUE,
    created_at timestamptz DEFAULT now()
);

CREATE TABLE orders (
    id bigint GENERATED ALWAYS AS IDENTITY,
    user_id bigint NOT NULL REFERENCES users (id),
    status text NOT NULL DEFAULT 'new; pending',
    PRIMARY KEY (id)
);

CREATE TABLE order_items (
    order_id bigint NOT NULL,
    sku text NOT NULL,
    qty integer CHECK (qty > 0),
    CONSTRAINT order_items_pkey PRIMARY KEY (order_id, sku)
);

ALTER TABLE ONLY order_items
    ADD CONSTRAINT order_items_order_fk FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE CASCADE;

CREATE FUNCTION touch() RETURNS trigger AS $$
BEGIN NEW.updated_at := now(); RETURN NEW; END;
$$ LANGUAGE plpgsql;

CREATE INDEX orders_user_idx ON orders (user_id);

CREATE TABLE audit_log (id int, payload jsonb);
CREATE TABLE broken (id int,,);
