-- The parts desk's schema and its demonstration stock.
--
--     $ createdb desk
--     $ psql -d desk -f examples/desk.sql
--     $ ply run examples/desk.ply --host --config-schema desk.config \
--         --set DESK_API_KEY=… --set DESK_DATABASE=postgres://localhost/desk
--
-- **This file is not the authority and must not become one.** `schema()` in
-- `examples/desk.ply` is, and the statements below are what `db::create_schema`
-- renders it to, written out so that a database can be created without a Ply
-- program having to be the thing that creates it — W4 ships a schema as a value
-- and refuses to ship a migration tool, and this is the honest shape of that
-- refusal.
--
-- Nothing checks the two against each other: a change to `schema()` and a change
-- to this file are one change by hand, and a drift surfaces as a failed statement
-- on whichever route touched it first. `examples/same-tests.sh` applies this file
-- and then runs the desk against both stores, which is where a drift is caught.
--
-- The `create table` text is `create_schema`'s, quoting and all, so a diff
-- between this file and that function is a diff a reader can do by eye.

create sequence "orders_id_seq";

create table "items" (
  "sku" text not null,
  "name" text not null,
  "price" numeric(12,2) not null,
  "on_hand" bigint not null,
  primary key ("sku"),
  -- The mechanical backstop under every drawdown. `place_order` reserves stock
  -- with a compare-and-set and rolls back when the update is refused; this is
  -- what refuses it, and it is refused by the one component in the stack that
  -- cannot be fooled by an annotation. `23514` is a value the program reads.
  constraint "items_on_hand_is_not_negative" check (on_hand >= 0)
);

create table "orders" (
  -- The desk does not choose order numbers. A `next_id` folded over the book is
  -- a check-then-act between two requests; a sequence is not.
  "id" bigint not null default nextval('orders_id_seq'),
  "customer" text not null,
  -- `lines` is a `List<Line>` and `state` is a sum, and `derive row` flattens
  -- neither: both are `jsonb` columns written through the json codec this
  -- service already derived for its wire format. W4 has no opinion about
  -- normalization — a desk that wanted `order_lines` as a table of its own
  -- would write two codecs and two statements.
  "lines" jsonb not null,
  "total" numeric(12,2) not null,
  "state" jsonb not null,
  primary key ("id")
);

-- The stock `seed_shelf` and `seed_orders` describe, inserted the way the
-- fixture in `desk.ply` inserts it: the orders name no id, so the sequence hands
-- out 1 and 2 and the next order placed is 3. Writing the ids here and leaving
-- the sequence at zero would make the first placement collide with `ada`'s
-- order, which is exactly the class of fixture bug a real database reports and a
-- hand-built double does not.
insert into "items" ("sku", "name", "price", "on_hand") values
  ('bolt', 'hex bolt', 0.40, 500),
  ('gasket', 'copper gasket', 2.25, 12),
  ('widget', 'left-handed widget', 12.50, 3);

insert into "orders" ("customer", "lines", "total", "state") values
  ('ada', '[{"qty":10,"sku":"bolt"}]', 4.00, '"Open"'),
  ('grace', '[{"qty":2,"sku":"gasket"}]', 4.50, '"Cancelled"');
