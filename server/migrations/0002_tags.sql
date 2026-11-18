-- Tags reduced to three: flag raising, concert, other.
-- "Линейка" (assembly) carried the flag icon and is where the flag is raised.
UPDATE streams SET category = 'flag' WHERE category = 'assembly';
UPDATE recordings SET category = 'flag' WHERE category = 'assembly';
UPDATE streams SET category = 'other' WHERE category NOT IN ('flag', 'concert', 'other');
UPDATE recordings SET category = 'other' WHERE category NOT IN ('flag', 'concert', 'other');
